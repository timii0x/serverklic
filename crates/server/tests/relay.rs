use quinn::{Connection, Endpoint};
use serverklic_protocol::*;
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command as Process, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};
struct Server {
    child: Child,
    dir: PathBuf,
    addr: std::net::SocketAddr,
    pin: String,
}
impl Server {
    fn start(password: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "serverklic-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut child = Process::new(env!("CARGO_BIN_EXE_serverklic-server"))
            .args([
                "--bind",
                "127.0.0.1:0",
                "--data-dir",
                dir.to_str().unwrap(),
                "--max-clients",
                "3",
            ])
            .env("SERVERKLIC_PASSWORD", password)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        let addr;
        loop {
            line.clear();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line.starts_with("ServerKlic listening on ") {
                addr = line.split_whitespace().nth(3).unwrap().parse().unwrap();
                break;
            }
        }
        let pin = fingerprint(&std::fs::read(dir.join("certificate.der")).unwrap());
        Self {
            child,
            dir,
            addr,
            pin,
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
struct Client {
    _ep: Endpoint,
    c: Connection,
    id: u64,
}
impl Client {
    async fn connect(server: &Server, name: &str) -> Self {
        let mut ep = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        ep.set_default_client_config(
            client_config(Some(server.pin.clone()), Arc::new(Mutex::new(None))).unwrap(),
        );
        let c = ep
            .connect(server.addr, "serverklic")
            .unwrap()
            .await
            .unwrap();
        send_json(
            &c,
            &Hello {
                version: VERSION,
                name: name.into(),
                password: "secret".into(),
            },
        )
        .await
        .unwrap();
        let Event::Welcome { id } = next(&c).await else {
            panic!("No welcome")
        };
        Self { _ep: ep, c, id }
    }
    async fn join(&self, room: u8) {
        send_json(&self.c, &Command::Join(room)).await.unwrap();
        self.state(room, false).await;
    }
    async fn state(&self, room: u8, muted: bool) {
        loop {
            if let Event::State(peers) = next(&self.c).await {
                if peers
                    .iter()
                    .any(|p| p.id == self.id && p.room == room && p.muted == muted)
                {
                    break;
                }
            }
        }
    }
    fn send_audio(&self, room: u8) {
        self.c
            .send_datagram(
                AudioPacket {
                    sender: 999,
                    room,
                    sequence: 1,
                    speaking: true,
                    opus: vec![1; 320],
                }
                .encode()
                .into(),
            )
            .unwrap();
    }
}
async fn next(c: &Connection) -> Event {
    tokio::time::timeout(Duration::from_secs(3), async {
        read_json(c.accept_uni().await.unwrap()).await.unwrap()
    })
    .await
    .unwrap()
}
async fn no_audio(c: &Connection) {
    assert!(
        tokio::time::timeout(Duration::from_millis(180), c.read_datagram())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn real_server_rooms_mute_pings_identity_and_instances() {
    let server = Server::start("secret");
    let other = Server::start("secret");
    let a = Client::connect(&server, "Alice").await;
    let b = Client::connect(&server, "Bob").await;
    let c = Client::connect(&server, "Cene").await;
    let isolated = Client::connect(&other, "Other instance").await;
    a.join(1).await;
    b.join(1).await;
    c.join(2).await;
    isolated.join(1).await;
    a.send_audio(1);
    let bytes = tokio::time::timeout(Duration::from_secs(2), b.c.read_datagram())
        .await
        .unwrap()
        .unwrap();
    let packet = AudioPacket::decode(&bytes).unwrap();
    assert_eq!(packet.sender, a.id);
    assert_eq!(packet.room, 1);
    assert_eq!(packet.opus.len(), 320);
    no_audio(&c.c).await;
    no_audio(&a.c).await;
    no_audio(&isolated.c).await;
    send_json(&a.c, &Command::Ping(42)).await.unwrap();
    loop {
        if let Event::Pong(n) = next(&a.c).await {
            assert_eq!(n, 42);
            break;
        }
    }
    a.c.send_datagram(ping_datagram(99, 1).into()).unwrap();
    let pong = a.c.read_datagram().await.unwrap();
    assert_eq!(decode_ping(&pong), Some((99, 1)));
    a.c.send_datagram(ping_datagram(100, 2).into()).unwrap();
    no_audio(&a.c).await;
    send_json(&a.c, &Command::Mute(true)).await.unwrap();
    a.state(1, true).await;
    a.send_audio(1);
    no_audio(&b.c).await;
    send_json(&a.c, &Command::Mute(false)).await.unwrap();
    a.state(1, false).await;
    b.join(2).await;
    a.send_audio(1);
    no_audio(&b.c).await;
    a.send_audio(2);
    no_audio(&c.c).await; // spoofing another room must fail
    a.join(0).await;
    a.send_audio(1);
    no_audio(&b.c).await;
    // Certificate changes are rejected before any credentials are sent.
    let mut ep = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    ep.set_default_client_config(
        client_config(Some("wrong".into()), Arc::new(Mutex::new(None))).unwrap(),
    );
    assert!(ep
        .connect(server.addr, "serverklic")
        .unwrap()
        .await
        .is_err());
    // Wrong password is rejected on a separate instance that still has capacity.
    ep.set_default_client_config(
        client_config(Some(other.pin.clone()), Arc::new(Mutex::new(None))).unwrap(),
    );
    let bad = ep.connect(other.addr, "serverklic").unwrap().await.unwrap();
    send_json(
        &bad,
        &Hello {
            version: VERSION,
            name: "Bad".into(),
            password: "wrong".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(next(&bad).await, Event::Error(_)));
    // Full server never admits a fourth user.
    ep.set_default_client_config(
        client_config(Some(server.pin.clone()), Arc::new(Mutex::new(None))).unwrap(),
    );
    let full = ep
        .connect(server.addr, "serverklic")
        .unwrap()
        .await
        .unwrap();
    send_json(
        &full,
        &Hello {
            version: VERSION,
            name: "Fourth".into(),
            password: "secret".into(),
        },
    )
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), full.accept_uni())
            .await
            .unwrap()
            .is_err()
    );
    b.c.close(0u32.into(), b"test leave");
    loop {
        if let Event::State(peers) = next(&c.c).await {
            if !peers.iter().any(|p| p.id == b.id) {
                break;
            }
        }
    }
}
