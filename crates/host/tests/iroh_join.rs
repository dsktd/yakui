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

    // The scene is retained and seeded on both; the copies converge to one.
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
