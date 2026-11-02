//! The bus across hosts (DESIGN.md, "Distributed: Iroh"): a [`Local`] bus whose own
//! messages go out on one iroh-gossip topic per space and whose subscribers get every other
//! host's. Retained topics converge by `(seq, origin)`: each host sends what it retains to
//! every new neighbour, and keeps a copy only if it is newer than its own.

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
/// what is retained here.
async fn carry(
    local: Local,
    mut tap: UnboundedReceiver<(String, Message)>,
    mut sender: GossipSender,
    mut receiver: GossipReceiver,
) {
    loop {
        tokio::select! {
            out = tap.recv() => {
                let Some((topic, msg)) = out else { return };
                send(&mut sender, false, topic, msg).await;
            }
            event = receiver.try_next() => match event {
                Ok(Some(Event::NeighborUp(peer))) => {
                    debug!("neighbour {} up", peer.fmt_short());
                    for (topic, msg) in local.snapshot() {
                        send(&mut sender, true, topic, msg).await;
                    }
                }
                Ok(Some(Event::Received(gossip))) => {
                    match serde_json::from_slice::<Wire>(&gossip.content) {
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
}
