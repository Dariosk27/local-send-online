//! Two real nodes on the loopback interface: direct connection, real
//! transfer, hash verification, resume from a partial file. Runs on every CI
//! OS (Linux, Windows, macOS).

use std::{path::PathBuf, time::Duration};

use lso_core::{
    config::TRANSFER_PROTOCOL,
    node::{Node, NodeConfig, Role},
    ticket::{self, Ticket},
    transfer, Keypair,
};

async fn node() -> Node {
    Node::start(NodeConfig {
        keypair: Keypair::generate_ed25519(),
        role: Role::Device,
        bootstrap: vec![],
        listen_port: 0,
        reachable: false,
        enable_mdns: false,
        enable_upnp: false,
        external_addrs: vec![],
    })
    .await
    .unwrap()
}

async fn loopback_ticket(n: &Node) -> Ticket {
    for _ in 0..50 {
        let s = n.snapshot().await.unwrap();
        let addrs: Vec<_> = s
            .listen_addrs
            .iter()
            .filter(|a| a.to_string().starts_with("/ip4/127.0.0.1"))
            .cloned()
            .collect();
        if addrs.len() >= 2 {
            return Ticket {
                peer: n.peer_id(),
                addrs,
            };
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("node did not start listening");
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lso-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn pseudo_random(len: usize) -> Vec<u8> {
    let mut x: u64 = 0x9e3779b97f4a7c15;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn transfer_and_resume_over_direct_connection() {
    let dir = tmpdir("xfer");
    let src = dir.join("data.bin");
    let content = pseudo_random(8 * 1024 * 1024 + 123);
    std::fs::write(&src, &content).unwrap();
    let dest = dir.join("in");

    let receiver = node().await;
    let sender = node().await;
    let mut incoming = receiver.control().accept(TRANSFER_PROTOCOL).unwrap();

    // Simulate an interrupted earlier attempt: 3 MiB already on disk.
    std::fs::create_dir_all(&dest).unwrap();
    let tag: String = sender.peer_id().to_base58().chars().rev().take(8).collect();
    let part = dest.join(format!(".data.bin.{}.{tag}.lso-part", content.len()));
    std::fs::write(&part, &content[..3 * 1024 * 1024]).unwrap();

    let dest2 = dest.clone();
    let recv = tokio::spawn(async move {
        use futures::StreamExt;
        let (peer, stream) = incoming.next().await.unwrap();
        let (offer, t) = transfer::read_offer(stream).await.unwrap();
        assert_eq!(offer.files[0].name, "data.bin");
        let mut resumed_from = None;
        let paths = t
            .accept(peer, &dest2, |p| {
                if let transfer::Progress::FileStart { offset, .. } = p {
                    resumed_from = Some(offset)
                }
            })
            .await
            .unwrap();
        (paths, resumed_from)
    });

    let target = loopback_ticket(&receiver).await;
    // Round-trip through the textual ticket, as a user would.
    let target = ticket::parse_target(&target.encode()).unwrap();
    let conn = sender
        .connect_direct(target)
        .await
        .expect("direct connection");
    assert!(!conn.addr.to_string().contains("p2p-circuit"));

    let mut control = sender.control();
    let mut last = 0;
    transfer::send_files(&mut control, receiver.peer_id(), &[src], "test", |p| {
        if let transfer::Progress::Bytes { done, .. } = p {
            assert!(done >= last, "progress must be monotonic");
            last = done;
        }
    })
    .await
    .unwrap();
    assert_eq!(last, content.len() as u64);

    let (paths, resumed_from) = tokio::time::timeout(Duration::from_secs(30), recv)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed_from, Some(3 * 1024 * 1024));
    assert_eq!(std::fs::read(&paths[0]).unwrap(), content);
    assert!(
        !part.exists(),
        "partial file must be renamed after verification"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ticket_roundtrip() {
    let t = Ticket {
        peer: Keypair::generate_ed25519().public().to_peer_id(),
        addrs: vec!["/ip4/1.2.3.4/udp/4001/quic-v1".parse().unwrap()],
    };
    let back = Ticket::decode(&t.encode()).unwrap();
    assert_eq!(back.peer, t.peer);
    assert_eq!(back.addrs, t.addrs);
    assert!(ticket::parse_target(&t.peer.to_base58()).is_ok());
    assert!(ticket::parse_target("garbage").is_err());
}
