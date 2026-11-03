//! The bus across hosts (DESIGN.md, "Distributed: Iroh"): a [`Local`] bus whose own
//! messages go out on one iroh-gossip topic per space and whose subscribers get every other
//! host's. Retained topics converge by `(seq, origin)`: each host sends what it retains to
//! every new neighbour, and keeps a copy only if it is newer than its own.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::time::Instant;

use anyhow::anyhow;
use futures_util::TryStreamExt;
use iroh::Endpoint;
use iroh::EndpointAddr;
use iroh::RelayMode;
use iroh::address_lookup::memory::MemoryLookup;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh_gossip::ALPN;
use iroh_gossip::TopicId;
use iroh_gossip::api::Event;
use iroh_gossip::api::GossipReceiver;
use iroh_gossip::api::GossipSender;
use iroh_gossip::net::Gossip;
use log::debug;
use log::warn;
use pane_api::BusStore;
use pane_api::Message;
use pane_api::PaneId;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

use crate::Result;
use crate::bus::Bus;
use crate::bus::Held;
use crate::bus::Local;

/// Sent to each new neighbour: `origin` is this host's id, for [`Local::authority`].
const HELLO: &str = "host/hello";

/// Room for the largest retained value plus its envelope.
const MAX_WIRE: usize = 512 * 1024;

/// How a space is joined: its gossip topic and the hosts to dial first. Shared as base32.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ticket {
    topic: TopicId,
    peers: Vec<EndpointAddr>,
}
impl fmt::Display for Ticket {
    fn fmt(
        &self,
        f: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        let json = serde_json::to_vec(self).map_err(|_| fmt::Error)?;
        f.write_str(
            &data_encoding::BASE32_NOPAD
                .encode(&json)
                .to_ascii_lowercase(),
        )
    }
}
impl FromStr for Ticket {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let json = data_encoding::BASE32_NOPAD
            .decode(s.to_ascii_uppercase().as_bytes())
            .map_err(|err| anyhow!("Ticket is not base32: {err}"))?;
        Ok(serde_json::from_slice(&json)?)
    }
}

/// Where the hosts find each other.
#[derive(Clone, Copy)]
pub enum Relays {
    /// iroh's public relays and address lookup: reaches hosts behind NAT.
    Default,
    /// Direct addresses from the ticket only, for one LAN or tests.
    Off,
}

/// One message on the wire: a topic and what [`Local`] would deliver for it.
#[derive(Serialize, Deserialize)]
struct Wire {
    topic: String,
    msg: Message,
}

/// A [`Local`] bus joined to other hosts over iroh.
pub struct IrohBus {
    local: Local,
    router: Router,
    topic: TopicId,
    relays: Relays,
    task: JoinHandle<()>,
}

impl IrohBus {
    /// Join the space of `ticket`, or start a new one with none, carrying `local`'s
    /// messages from now on.
    pub async fn join(
        local: Local,
        ticket: Option<Ticket>,
        relays: Relays,
    ) -> Result<Self> {
        let lookup = MemoryLookup::new();
        let builder = match relays {
            Relays::Default => Endpoint::builder(presets::N0),
            Relays::Off => Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled),
        };
        let endpoint = builder.address_lookup(lookup.clone()).bind().await?;
        let gossip = Gossip::builder()
            .max_message_size(MAX_WIRE)
            .spawn(endpoint.clone());
        let router = Router::builder(endpoint)
            .accept(ALPN, gossip.clone())
            .spawn();
        let (topic, peers) = match ticket {
            Some(ticket) => (ticket.topic, ticket.peers),
            None => (TopicId::from_bytes(rand::random()), Vec::new()),
        };
        let ids = peers.iter().map(|peer| peer.id).collect();
        for peer in peers {
            lookup.add_endpoint_info(peer);
        }
        let (sender, receiver) = gossip.subscribe(topic, ids).await?.split();
        let task = tokio::spawn(carry(local.clone(), local.tap(), sender, receiver));
        Ok(Self {
            local,
            router,
            topic,
            relays,
            task,
        })
    }
    /// A ticket for another host to join this space through this one.
    pub async fn ticket(&self) -> Ticket {
        let endpoint = self.router.endpoint();
        if let Relays::Default = self.relays {
            // Wait for a home relay, so the ticket reaches this host from behind NAT.
            endpoint.online().await;
        }
        Ticket {
            topic: self.topic,
            peers: vec![endpoint.addr()],
        }
    }
    pub async fn shutdown(self) -> Result<()> {
        self.task.abort();
        self.router.shutdown().await?;
        Ok(())
    }
}

/// Send what `local` publishes, deliver what other hosts do, and hand every new neighbour
/// what is retained here. Neighbours say who they are by [`HELLO`], so `local` knows which
/// hosts share the space.
async fn carry(
    local: Local,
    mut tap: UnboundedReceiver<(String, Message)>,
    mut sender: GossipSender,
    mut receiver: GossipReceiver,
) {
    let mut hosts = HashMap::new();
    loop {
        tokio::select! {
            out = tap.recv() => {
                let Some((topic, msg)) = out else { return };
                send(&mut sender, false, topic, msg).await;
            }
            event = receiver.try_next() => match event {
                Ok(Some(Event::NeighborUp(peer))) => {
                    debug!("neighbour {} up", peer.fmt_short());
                    let hello = Message {
                        value: Value::Null,
                        origin: local.origin(),
                        seq: None,
                        ts: None,
                    };
                    send(&mut sender, true, HELLO.to_owned(), hello).await;
                    for (topic, msg) in local.snapshot() {
                        send(&mut sender, true, topic, msg).await;
                    }
                }
                Ok(Some(Event::NeighborDown(peer))) => {
                    debug!("neighbour {} down", peer.fmt_short());
                    if let Some(host) = hosts.remove(&peer) {
                        local.peer_down(host);
                    }
                }
                Ok(Some(Event::Received(gossip))) => {
                    match serde_json::from_slice::<Wire>(&gossip.content) {
                        Ok(wire) if wire.topic == HELLO => {
                            hosts.insert(gossip.delivered_from, wire.msg.origin);
                            local.peer_up(wire.msg.origin);
                        }
                        Ok(wire) => {
                            local.deliver(&wire.topic, wire.msg);
                        }
                        Err(err) => warn!("dropped a gossip message: {err}"),
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => return,
                Err(err) => {
                    warn!("gossip ended: {err}");
                    return;
                }
            },
        }
    }
}

async fn send(
    sender: &mut GossipSender,
    neighbours: bool,
    topic: String,
    msg: Message,
) {
    let bytes = match serde_json::to_vec(&Wire { topic, msg }) {
        Ok(bytes) => bytes,
        Err(err) => return warn!("could not encode a message: {err}"),
    };
    let sent = match neighbours {
        true => sender.broadcast_neighbors(bytes.into()).await,
        false => sender.broadcast(bytes.into()).await,
    };
    if let Err(err) = sent {
        warn!("could not send a message: {err}");
    }
}

impl Drop for IrohBus {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Bus for IrohBus {
    fn retained(
        &self,
        topic: &str,
    ) -> Option<Message> {
        self.local.retained(topic)
    }
    fn unsubscribe(
        &self,
        pane: PaneId,
    ) {
        self.local.unsubscribe(pane);
    }
    fn release(
        &self,
        pane: PaneId,
    ) {
        self.local.release(pane);
    }
    fn hold(
        &self,
        pane: PaneId,
    ) -> Held {
        self.local.hold(pane)
    }
    fn restore(
        &self,
        pane: PaneId,
        held: Held,
    ) {
        self.local.restore(pane, held);
    }
    fn take_queued(&self) -> Vec<(PaneId, String, Message)> {
        self.local.take_queued()
    }
    fn next_due(&self) -> Option<Instant> {
        self.local.next_due()
    }
    fn set_kind(
        &self,
        pane: PaneId,
        kind: &str,
    ) {
        self.local.set_kind(pane, kind);
    }
    fn save(&self) -> Result<()> {
        self.local.save()
    }
}

impl BusStore for IrohBus {
    fn subscribe(
        &self,
        pane: PaneId,
        topic: &str,
    ) {
        self.local.subscribe(pane, topic);
    }
    fn publish(
        &self,
        pane: PaneId,
        topic: &str,
        value: Value,
        retain: bool,
    ) {
        self.local.publish(pane, topic, value, retain);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use pane_api::HostId;
    use pane_api::protocol::Topic;
    use serde_json::json;

    use super::*;

    fn local(host: u128) -> (Local, PathBuf) {
        let dir = std::env::temp_dir().join(format!("yakui-iroh-{:08x}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        (
            Local::open(dir.join("bus.json"), HostId(host)).unwrap(),
            dir,
        )
    }

    /// Wait up to ten seconds for `done`.
    async fn until(mut done: impl FnMut() -> bool) {
        for _ in 0..100 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("timed out");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn two_hosts_share_a_space() {
        let (a_local, a_dir) = local(1);
        let (b_local, b_dir) = local(2);
        let a_pane = PaneId::new(HostId(1), 1);
        let b_pane = PaneId::new(HostId(2), 1);
        a_local.set_kind(a_pane, "view2d");
        a_local.publish(a_pane, "view2d/view", json!({ "zoom": 2 }), true);

        let a = IrohBus::join(a_local, None, Relays::Off).await.unwrap();
        let ticket: Ticket = a.ticket().await.to_string().parse().unwrap();
        let b = IrohBus::join(b_local, Some(ticket), Relays::Off)
            .await
            .unwrap();

        // Retained before b joined: b converges to a's copy.
        until(|| b.retained("view2d/view").is_some()).await;
        let view = b.retained("view2d/view").unwrap();
        assert_eq!((view.value, view.origin), (json!({ "zoom": 2 }), HostId(1)));

        // A transient message reaches the other host's subscriber, once.
        b.subscribe(b_pane, Topic::HOVER);
        a.subscribe(a_pane, Topic::HOVER);
        a.take_queued();
        a.publish(a_pane, Topic::HOVER, json!({ "id": "seed-box" }), false);
        let mut got = Vec::new();
        until(|| {
            got.extend(b.take_queued());
            !got.is_empty()
        })
        .await;
        assert_eq!(got[0].0, b_pane);
        assert_eq!(got[0].1, Topic::HOVER);
        assert_eq!(got[0].2.origin, HostId(1));

        // A newer write on b wins on a too.
        b.set_kind(b_pane, "view2d");
        b.publish(b_pane, "view2d/view", json!({ "zoom": 5 }), true);
        until(|| a.retained("view2d/view").unwrap().value == json!({ "zoom": 5 })).await;
        assert_eq!(a.retained("view2d/view"), b.retained("view2d/view"));

        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
        std::fs::remove_dir_all(a_dir).unwrap();
        std::fs::remove_dir_all(b_dir).unwrap();
    }

    fn grab(
        bus: &IrohBus,
        pane: PaneId,
        id: &str,
    ) {
        let cmd = json!({ "cmd": "grab", "id": id });
        bus.publish(pane, Topic::CMD, cmd, false);
    }

    fn holder(
        bus: &IrohBus,
        id: &str,
    ) -> Option<PaneId> {
        let owners = bus.retained(Topic::OWNERS)?;
        pane_api::protocol::Owners::from_message(&owners.value).holder(id)
    }

    /// Whether `pane` was told its command was rejected, among what is queued on `bus`.
    fn rejected(
        bus: &IrohBus,
        pane: PaneId,
    ) -> bool {
        let topic = Topic::rejected(pane);
        bus.take_queued().iter().any(|(_pane, t, _msg)| *t == topic)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn one_of_two_concurrent_grabs_wins() {
        let (a_local, a_dir) = local(1);
        let (b_local, b_dir) = local(2);
        a_local.seed();
        let a_pane = PaneId::new(HostId(1), 1);
        let b_pane = PaneId::new(HostId(2), 1);
        let a = IrohBus::join(a_local, None, Relays::Off).await.unwrap();
        let ticket = a.ticket().await;
        let b = IrohBus::join(b_local, Some(ticket), Relays::Off)
            .await
            .unwrap();
        until(|| b.local.authority() == HostId(1) && b.retained(Topic::SCENE).is_some()).await;
        a.subscribe(a_pane, &Topic::rejected(a_pane));
        b.subscribe(b_pane, &Topic::rejected(b_pane));

        grab(&b, b_pane, "seed-box");
        grab(&a, a_pane, "seed-box");
        // The authority decides in arrival order: exactly one pane holds it on both hosts,
        // and the other is told so.
        let (mut a_lost, mut b_lost) = (false, false);
        until(|| {
            a_lost |= rejected(&a, a_pane);
            b_lost |= rejected(&b, b_pane);
            holder(&a, "seed-box").is_some()
                && holder(&a, "seed-box") == holder(&b, "seed-box")
                && a_lost != b_lost
        })
        .await;
        let winner = holder(&a, "seed-box").unwrap();
        assert_eq!(winner, if a_lost { b_pane } else { a_pane });

        a.shutdown().await.unwrap();
        b.shutdown().await.unwrap();
        std::fs::remove_dir_all(a_dir).unwrap();
        std::fs::remove_dir_all(b_dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn another_host_takes_over_when_the_authority_leaves() {
        let (a_local, a_dir) = local(1);
        let (b_local, b_dir) = local(2);
        let (c_local, c_dir) = local(3);
        a_local.seed();
        let a_pane = PaneId::new(HostId(1), 1);
        let b_pane = PaneId::new(HostId(2), 1);
        let c_pane = PaneId::new(HostId(3), 1);
        let a = IrohBus::join(a_local, None, Relays::Off).await.unwrap();
        let b = IrohBus::join(b_local, Some(a.ticket().await), Relays::Off)
            .await
            .unwrap();
        let mut ticket = a.ticket().await;
        ticket.peers.extend(b.ticket().await.peers);
        let c = IrohBus::join(c_local, Some(ticket), Relays::Off)
            .await
            .unwrap();
        until(|| {
            [&b, &c]
                .iter()
                .all(|bus| bus.local.authority() == HostId(1))
                && c.local.snapshot().len() == a.local.snapshot().len()
        })
        .await;

        grab(&a, a_pane, "seed-ball");
        grab(&b, b_pane, "seed-box");
        until(|| {
            [&a, &b, &c].iter().all(|bus| {
                holder(bus, "seed-box") == Some(b_pane) && holder(bus, "seed-ball") == Some(a_pane)
            })
        })
        .await;

        // a leaves: b is the lowest left, keeps b's grab and drops a's.
        a.shutdown().await.unwrap();
        until(|| {
            [&b, &c]
                .iter()
                .all(|bus| bus.local.authority() == HostId(2))
        })
        .await;
        until(|| {
            [&b, &c]
                .iter()
                .all(|bus| holder(bus, "seed-ball").is_none())
        })
        .await;
        assert_eq!(holder(&c, "seed-box"), Some(b_pane));

        // Grabs still work through b: c gets a free object and is refused a held one.
        c.subscribe(c_pane, &Topic::rejected(c_pane));
        grab(&c, c_pane, "seed-ball");
        until(|| {
            [&b, &c]
                .iter()
                .all(|bus| holder(bus, "seed-ball") == Some(c_pane))
        })
        .await;
        grab(&c, c_pane, "seed-box");
        until(|| rejected(&c, c_pane)).await;
        assert_eq!(holder(&b, "seed-box"), Some(b_pane));

        b.shutdown().await.unwrap();
        c.shutdown().await.unwrap();
        for dir in [a_dir, b_dir, c_dir] {
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
