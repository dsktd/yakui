//! Two hosts share a space by ticket, relays off.
#![cfg(feature = "iroh")]

use std::path::PathBuf;
use std::time::Duration;

use host::host::Host;
use host::iroh_bus::Relays;
use host::registry::Registry;
use host::space::SpaceId;

fn host() -> (Host, PathBuf) {
    let dir = std::env::temp_dir().join(format!("yakui-join-{:08x}", rand::random::<u32>()));
    let mut registry = Registry::default();
    registry.add::<pane_clock::Clock>();
    registry.add::<pane_notes::Notes>();
    (Host::open(&dir, registry).unwrap(), dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn second_host_joins_by_ticket() {
    let (a, a_dir) = host();
    let (b, b_dir) = host();
    let a_space = SpaceId::random();
    let b_space = SpaceId::random();

    let ticket = a.share(&a_space, None, Relays::Off).await.unwrap();
    assert_eq!(a.ticket(&a_space), Some(ticket.to_string()));
    let ticket = ticket.to_string().parse().unwrap();
    b.share(&b_space, Some(ticket), Relays::Off).await.unwrap();

    // The scene is seeded on a; b takes a's copy.
    let a_space = a.space(&a_space).unwrap();
    let b_space = b.space(&b_space).unwrap();
    for _ in 0..100 {
        let (x, y) = (a_space.retained("scene"), b_space.retained("scene"));
        if x.is_some() && x == y {
            std::fs::remove_dir_all(a_dir).unwrap();
            std::fs::remove_dir_all(b_dir).unwrap();
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("scene did not converge");
}

#[tokio::test(flavor = "multi_thread")]
async fn joining_host_takes_the_shared_scene() {
    let (a, a_dir) = host();
    let (b, b_dir) = host();
    let a_space = SpaceId::random();
    let b_space = SpaceId::random();
    // b once had this space on its own, an empty scene far along in seq.
    let old = b_dir.join(b_space.as_str());
    std::fs::create_dir_all(&old).unwrap();
    let stale = r#"{ "scene": { "value": { "objects": [] },
        "origin": "00000000000000000000000000000002", "seq": 99 } }"#;
    std::fs::write(old.join("bus.json"), stale).unwrap();

    let ticket = a.share(&a_space, None, Relays::Off).await.unwrap();
    let ticket = ticket.to_string().parse().unwrap();
    b.share(&b_space, Some(ticket), Relays::Off).await.unwrap();

    // Both end on a's seeded scene, not b's old one.
    let a_space = a.space(&a_space).unwrap();
    let b_space = b.space(&b_space).unwrap();
    let objects = |scene: &serde_json::Value| scene["objects"].as_array().map(Vec::len);
    for _ in 0..100 {
        let (x, y) = (a_space.retained("scene"), b_space.retained("scene"));
        if let Some(x) = x
            && Some(&x) == y.as_ref()
        {
            assert_eq!(objects(&x.value), Some(4));
            std::fs::remove_dir_all(a_dir).unwrap();
            std::fs::remove_dir_all(b_dir).unwrap();
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("scene did not converge");
}

#[tokio::test(flavor = "multi_thread")]
async fn space_config_picks_the_bus() {
    let (a, a_dir) = host();
    let shared = SpaceId::random();
    let local = SpaceId::random();
    let skipped = SpaceId::random();
    for (id, config) in [
        (&shared, r#"{ "bus": "iroh", "relays": false }"#),
        (&local, r#"{ "bus": "local" }"#),
        (&skipped, r#"{ "bus": "iroh", "relays": false }"#),
    ] {
        let dir = a_dir.join(id.as_str());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("space.json"), config).unwrap();
    }

    let tickets = a.share_configured(Some(&skipped)).await.unwrap();
    assert_eq!(tickets.len(), 1);
    assert_eq!(tickets[0].0, shared);
    assert!(a.ticket(&shared).is_some());
    assert!(a.ticket(&local).is_none());
    assert!(a.ticket(&skipped).is_none());
    std::fs::remove_dir_all(a_dir).unwrap();
}
