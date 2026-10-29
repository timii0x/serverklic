use anyhow::{bail, Context, Result};
use serverklic_protocol::*;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc as tmpsc;
pub struct UiEvent {
    pub session: Arc<AtomicBool>,
    pub kind: UiKind,
}
pub enum UiKind {
    Trust(String),
    Connected {
        id: u64,
        outgoing: tmpsc::Sender<AudioPacket>,
        incoming: mpsc::Receiver<AudioPacket>,
    },
    State(Vec<Peer>),
    Ping(f64),
    VoicePing(f64),
    Speaking(u64),
    AudioError(String),
    Error(String),
}
pub fn emit(tx: &mpsc::Sender<UiEvent>, session: &Arc<AtomicBool>, kind: UiKind) {
    let _ = tx.send(UiEvent {
        session: session.clone(),
        kind,
    });
}
pub fn start(
    address: String,
    name: String,
    password: String,
    pin: Option<String>,
    stop: Arc<AtomicBool>,
    commands: tmpsc::Receiver<Command>,
    ui: mpsc::Sender<UiEvent>,
) {
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(anyhow::Error::from)
            .and_then(|rt| {
                rt.block_on(run(
                    address,
                    name,
                    password,
                    pin,
                    stop.clone(),
                    commands,
                    &ui,
                ))
            });
        if let Err(e) = result {
            if !stop.load(Ordering::Relaxed) {
                emit(
                    &ui,
                    &stop,
                    UiKind::Error(format!("Povezava prekinjena: {e:#}")),
                );
            }
        }
    });
}
async fn run(
    address: String,
    name: String,
    password: String,
    pin: Option<String>,
    stop: Arc<AtomicBool>,
    mut commands: tmpsc::Receiver<Command>,
    ui: &mpsc::Sender<UiEvent>,
) -> Result<()> {
    let addr = tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host(&address))
        .await??
        .next()
        .context("Naslov ni najden")?;
    let mut endpoint = quinn::Endpoint::client(
        if addr.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        }
        .parse()?,
    )?;
    let observed = Arc::new(Mutex::new(None));
    endpoint.set_default_client_config(client_config(pin.clone(), observed.clone())?);
    let conn = tokio::time::timeout(
        Duration::from_secs(8),
        endpoint.connect(addr, "serverklic")?,
    )
    .await??;
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    if pin.is_none() {
        emit(
            ui,
            &stop,
            UiKind::Trust(
                observed
                    .lock()
                    .unwrap()
                    .clone()
                    .context("Manjka certifikat")?,
            ),
        );
        conn.close(0u32.into(), b"Certificate discovery");
        return Ok(());
    }
    send_json(
        &conn,
        &Hello {
            version: VERSION,
            name,
            password,
        },
    )
    .await?;
    let welcome: Event = tokio::time::timeout(Duration::from_secs(8), async {
        read_json(conn.accept_uni().await?).await
    })
    .await??;
    let id = match welcome {
        Event::Welcome { id } => id,
        Event::Error(e) => bail!("{e}"),
        _ => bail!("Neveljaven odgovor strežnika"),
    };
    let (outgoing, mut out) = tmpsc::channel::<AudioPacket>(8);
    let (incoming, in_rx) = mpsc::sync_channel::<AudioPacket>(128);
    emit(
        ui,
        &stop,
        UiKind::Connected {
            id,
            outgoing,
            incoming: in_rx,
        },
    );
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let start = Instant::now();
    let mut last_ping = Instant::now() - Duration::from_secs(2);
    let mut pending = None;
    let mut voice_pending = None;
    let mut room = 0;
    loop {
        tokio::select! {
            _=ticker.tick()=> {
                if stop.load(Ordering::Relaxed){break;}
                if last_ping.elapsed()>=Duration::from_secs(1){
                    last_ping=Instant::now();let nonce=start.elapsed().as_micros() as u64;
                    pending=Some((nonce,Instant::now()));send_json(&conn,&Command::Ping(nonce)).await?;
                    if room>0 {voice_pending=Some((nonce,room,Instant::now()));let _=conn.send_datagram(ping_datagram(nonce,room).into());}
                }
            }
            cmd=commands.recv()=> {let Some(cmd)=cmd else {break};send_json(&conn,&cmd).await?;}
            data=conn.accept_uni()=> {
                match read_json::<Event>(data?).await? {
                    Event::State(peers)=>{room=peers.iter().find(|p|p.id==id).map_or(0,|p|p.room);emit(ui,&stop,UiKind::State(peers));}
                    Event::Pong(n)=>if let Some((expected,t))=pending {if n==expected{emit(ui,&stop,UiKind::Ping(t.elapsed().as_secs_f64()*1000.0));pending=None;}},
                    Event::Error(e)=>bail!("{e}"),_=>{},
                }
            }
            data=conn.read_datagram()=> {
                let data=data?;
                if let Some((nonce,r))=decode_ping(&data){
                    if let Some((expected,expected_room,t))=voice_pending {if nonce==expected&&r==expected_room&&r==room{emit(ui,&stop,UiKind::VoicePing(t.elapsed().as_secs_f64()*1000.0));voice_pending=None;}}
                }else if let Some(p)=AudioPacket::decode(&data){
                    if p.room==room {if p.speaking{emit(ui,&stop,UiKind::Speaking(p.sender));}let _=incoming.try_send(p);}
                }
            }
            Some(mut p)=out.recv()=>{if p.room==room&&room>0 {p.sender=id;if p.speaking{emit(ui,&stop,UiKind::Speaking(id));}let _=conn.send_datagram(p.encode().into());}}
        }
    }
    conn.close(0u32.into(), b"Bye");
    Ok(())
}
