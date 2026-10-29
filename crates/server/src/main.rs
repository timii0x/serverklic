use anyhow::{bail, Context, Result};
use clap::Parser;
use quinn::{Connection, Endpoint};
use serverklic_protocol::*;
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

#[derive(Parser)]
#[command(about = "ServerKlic — 4 voice rooms, Opus 128 kb/s over encrypted QUIC")]
struct Args {
    #[arg(long, default_value = "0.0.0.0:5000")]
    bind: SocketAddr,
    #[arg(long, default_value = "server-data")]
    data_dir: PathBuf,
    #[arg(long, default_value_t=32, value_parser=clap::value_parser!(u16).range(1..=32))]
    max_clients: u16,
}
struct Client {
    peer: Peer,
    conn: Connection,
    tx: mpsc::Sender<Event>,
}
type Clients = Arc<Mutex<BTreeMap<u64, Client>>>;
fn broadcast(clients: &BTreeMap<u64, Client>) {
    let state = Event::State(clients.values().map(|c| c.peer.clone()).collect());
    for c in clients.values() {
        if c.tx.try_send(state.clone()).is_err() {
            c.conn.close(1u32.into(), b"Client too slow");
        }
    }
}
fn server_config(args: &Args) -> Result<quinn::ServerConfig> {
    std::fs::create_dir_all(&args.data_dir)?;
    let cert_path = args.data_dir.join("certificate.der");
    let key_path = args.data_dir.join("private-key.der");
    if !cert_path.exists() && !key_path.exists() {
        let cert = rcgen::generate_simple_self_signed(vec!["serverklic".into()])?;
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        opts.open(&key_path)?
            .write_all(&cert.key_pair.serialize_der())?;
        std::fs::write(&cert_path, cert.cert.der())?;
    }
    let cert = std::fs::read(cert_path).context("Read server certificate")?;
    println!("SHA-256 fingerprint: {}", fingerprint(&cert));
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(std::fs::read(key_path)?);
    let mut config = quinn::ServerConfig::with_single_cert(vec![cert.into()], key.into())?;
    config.transport_config(transport());
    Ok(config)
}
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let args = Args::parse();
    let password = Arc::new(std::env::var("SERVERKLIC_PASSWORD").unwrap_or_default());
    let endpoint = Endpoint::server(server_config(&args)?, args.bind)?;
    println!(
        "ServerKlic listening on {} (UDP), 4 rooms, max {} clients",
        endpoint.local_addr()?,
        args.max_clients
    );
    if password.is_empty() {
        println!("Access: open. Set SERVERKLIC_PASSWORD to require a password.");
    }
    let clients: Clients = Arc::default();
    let ids = AtomicU64::new(1);
    let pending = Arc::new(tokio::sync::Semaphore::new(64));
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break };
                let Ok(permit) = pending.clone().try_acquire_owned() else { incoming.refuse(); continue };
                let clients = clients.clone(); let password = password.clone();
                let id = ids.fetch_add(1,Ordering::Relaxed); let max = args.max_clients as usize;
                tokio::spawn(async move {
                    if let Ok(Ok(conn)) = tokio::time::timeout(Duration::from_secs(8), incoming).await {
                        if let Err(e) = serve(conn.clone(), clients.clone(), id, max, &password, permit).await {
                            eprintln!("Client {id}: {e}");
                        }
                        conn.close(0u32.into(), b"Disconnected");
                        let mut c = clients.lock().unwrap();
                        if c.remove(&id).is_some() { broadcast(&c); }
                    }
                });
            }
            _ = tokio::signal::ctrl_c() => { break; }
        }
    }
    endpoint.close(0u32.into(), b"Server stopped");
    endpoint.wait_idle().await;
    Ok(())
}
async fn serve(
    conn: Connection,
    clients: Clients,
    id: u64,
    max: usize,
    password: &str,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<()> {
    let hello: Hello = tokio::time::timeout(Duration::from_secs(8), async {
        read_json(conn.accept_uni().await?).await
    })
    .await??;
    if hello.version != VERSION || !valid_name(&hello.name) || hello.password != password {
        send_json(
            &conn,
            &Event::Error("Neveljavno ime, geslo ali različica aplikacije.".into()),
        )
        .await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        bail!("Rejected login");
    }
    let (tx, mut rx) = mpsc::channel(64);
    {
        let mut c = clients.lock().unwrap();
        if c.len() >= max {
            bail!("Server is full");
        }
        tx.try_send(Event::Welcome { id })?;
        c.insert(
            id,
            Client {
                peer: Peer {
                    id,
                    name: hello.name.trim().into(),
                    room: 0,
                    muted: false,
                },
                conn: conn.clone(),
                tx,
            },
        );
        broadcast(&c);
    }
    drop(permit);
    let writer = async {
        while let Some(event) = rx.recv().await {
            send_json(&conn, &event).await?;
        }
        Ok::<_, anyhow::Error>(())
    };
    let reader = async {
        let mut window = Instant::now();
        let mut count = 0;
        loop {
            let cmd: Command = read_json(conn.accept_uni().await?).await?;
            if window.elapsed() >= Duration::from_secs(1) {
                window = Instant::now();
                count = 0;
            }
            count += 1;
            if count > 30 {
                bail!("Too many commands");
            }
            let mut c = clients.lock().unwrap();
            let Some(me) = c.get_mut(&id) else { break };
            match cmd {
                Command::Join(room) if room <= 4 => {
                    me.peer.room = room;
                    broadcast(&c);
                }
                Command::Mute(muted) => {
                    me.peer.muted = muted;
                    broadcast(&c);
                }
                Command::Ping(n) => {
                    let _ = me.tx.try_send(Event::Pong(n));
                }
                _ => bail!("Invalid room"),
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    let audio = async {
        let mut window = Instant::now();
        let mut count = 0;
        loop {
            let data = conn.read_datagram().await?;
            if window.elapsed() >= Duration::from_secs(1) {
                window = Instant::now();
                count = 0;
            }
            count += 1;
            if count > 100 {
                bail!("Too many datagrams");
            }
            let c = clients.lock().unwrap();
            let Some(me) = c.get(&id) else { break };
            if let Some((_, room)) = decode_ping(&data) {
                if room > 0 && room == me.peer.room {
                    let _ = conn.send_datagram(data);
                }
                continue;
            }
            if let Some(mut p) = AudioPacket::decode(&data) {
                if me.peer.muted || me.peer.room == 0 || p.room != me.peer.room {
                    continue;
                }
                p.sender = id;
                let bytes = p.encode();
                for (other_id, other) in c.iter() {
                    if *other_id != id && other.peer.room == p.room {
                        let _ = other.conn.send_datagram(bytes.clone().into());
                    }
                }
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    tokio::select! { r=writer => r, r=reader => r, r=audio => r }
}
