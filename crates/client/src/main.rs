mod audio;
mod network;
use eframe::egui;
use serde::{Deserialize, Serialize};
use serverklic_protocol::*;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

#[derive(Default, Serialize, Deserialize)]
struct Preferences {
    name: String,
    address: String,
    pins: BTreeMap<String, String>,
}
struct App {
    prefs: Preferences,
    password: String,
    status: String,
    connecting: bool,
    trust: Option<String>,
    peers: Vec<Peer>,
    id: Option<u64>,
    commands: Option<tokio::sync::mpsc::Sender<Command>>,
    events: mpsc::Receiver<network::UiEvent>,
    event_tx: mpsc::Sender<network::UiEvent>,
    stop: Arc<AtomicBool>,
    room: Arc<AtomicU8>,
    muted: Arc<AtomicBool>,
    ping: Option<(f64, Instant)>,
    voice_ping: Option<(f64, Instant)>,
    speaking: BTreeMap<u64, Instant>,
    audio: Option<audio::AudioEngine>,
    audio_channels: Option<(
        tokio::sync::mpsc::Sender<AudioPacket>,
        mpsc::Receiver<AudioPacket>,
    )>,
}
impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut prefs: Preferences = cc
            .storage
            .and_then(|s| eframe::get_value(s, "preferences"))
            .unwrap_or_default();
        if prefs.address.is_empty() {
            prefs.address = "127.0.0.1:5000".into();
        }
        let (event_tx, events) = mpsc::channel();
        Self {
            prefs,
            password: String::new(),
            status: "Pripravljen na povezavo".into(),
            connecting: false,
            trust: None,
            peers: vec![],
            id: None,
            commands: None,
            events,
            event_tx,
            stop: Arc::new(AtomicBool::new(false)),
            room: Arc::new(AtomicU8::new(0)),
            muted: Arc::new(AtomicBool::new(false)),
            ping: None,
            voice_ping: None,
            speaking: BTreeMap::new(),
            audio: None,
            audio_channels: None,
        }
    }
    fn connect(&mut self) {
        if !valid_name(&self.prefs.name) {
            self.status = "Vpiši ime (1–32 znakov).".into();
            return;
        }
        self.disconnect();
        self.connecting = true;
        self.trust = None;
        self.status = "Povezovanje …".into();
        self.stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        self.commands = Some(tx);
        network::start(
            self.prefs.address.trim().to_owned(),
            self.prefs.name.trim().to_owned(),
            self.password.clone(),
            self.prefs.pins.get(self.prefs.address.trim()).cloned(),
            self.stop.clone(),
            rx,
            self.event_tx.clone(),
        );
    }
    fn disconnect(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.room.store(0, Ordering::Relaxed);
        self.muted.store(false, Ordering::Relaxed);
        self.audio = None;
        self.audio_channels = None;
        self.commands = None;
        self.id = None;
        self.peers.clear();
        self.speaking.clear();
        self.ping = None;
        self.voice_ping = None;
        self.connecting = false;
        // All events from the old worker are tagged by its stop flag in network.rs.
    }
    fn command(&mut self, c: Command) {
        if self
            .commands
            .as_ref()
            .is_some_and(|tx| tx.try_send(c).is_err())
        {
            self.status = "Povezava je zasedena; poskusi ponovno.".into();
        }
    }
    fn join(&mut self, room: u8) {
        if room > 0 && self.audio.is_none() {
            let Some((outgoing, incoming)) = self.audio_channels.take() else {
                self.status = "Preveri mikrofon in ponovno poveži aplikacijo.".into();
                return;
            };
            match audio::AudioEngine::start(
                self.room.clone(),
                self.muted.clone(),
                outgoing,
                incoming,
                self.event_tx.clone(),
                self.stop.clone(),
            ) {
                Ok(a) => self.audio = Some(a),
                Err(e) => {
                    self.status = format!("Zvok ni pripravljen: {e:#}. Ponovno poveži.");
                    return;
                }
            }
        }
        self.voice_ping = None;
        // Gate audio locally until the server confirms room membership.
        self.room.store(0, Ordering::Relaxed);
        self.command(Command::Join(room));
    }
    fn poll(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            if !Arc::ptr_eq(&event.session, &self.stop) {
                continue;
            }
            match event.kind {
                network::UiKind::Trust(fp) => {
                    self.trust = Some(fp);
                    self.connecting = false;
                    self.commands = None;
                    self.status = "Potrdi identiteto strežnika.".into();
                }
                network::UiKind::Connected {
                    id,
                    outgoing,
                    incoming,
                } => {
                    self.id = Some(id);
                    self.connecting = false;
                    self.status = "Povezan • izberi klic".into();
                    self.audio_channels = Some((outgoing, incoming));
                }
                network::UiKind::State(peers) => {
                    if let Some(me) = peers.iter().find(|p| Some(p.id) == self.id) {
                        self.room.store(me.room, Ordering::Relaxed);
                    }
                    self.speaking
                        .retain(|id, _| peers.iter().any(|p| p.id == *id));
                    self.peers = peers;
                }
                network::UiKind::Ping(ms) => self.ping = Some((ms, Instant::now())),
                network::UiKind::VoicePing(ms) => self.voice_ping = Some((ms, Instant::now())),
                network::UiKind::Speaking(id) => {
                    self.speaking.insert(id, Instant::now());
                }
                network::UiKind::AudioError(s) => {
                    self.status = s;
                    self.join(0);
                    self.audio = None;
                }
                network::UiKind::Error(e) => {
                    self.disconnect();
                    self.status = e;
                }
            }
        }
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll();
        ctx.request_repaint_after(Duration::from_millis(100));
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(12.0);
                ui.heading(egui::RichText::new("ServerKlic").size(30.0));
                ui.label("Tvoj strežnik. Štirje klici. En klik.");
                ui.add_space(16.0);
                ui.add_enabled_ui(self.id.is_none() && !self.connecting, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_sized([145.0, 26.0], egui::Label::new("Tvoje ime"));
                        ui.add_sized(
                            [300.0, 26.0],
                            egui::TextEdit::singleline(&mut self.prefs.name),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized([145.0, 26.0], egui::Label::new("IP / domena : port"));
                        ui.add_sized(
                            [300.0, 26.0],
                            egui::TextEdit::singleline(&mut self.prefs.address),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.add_sized([145.0, 26.0], egui::Label::new("Geslo (neobvezno)"));
                        ui.add_sized(
                            [300.0, 26.0],
                            egui::TextEdit::singleline(&mut self.password).password(true),
                        );
                    });
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if self.id.is_none() && !self.connecting {
                        if ui.button("Poveži se").clicked() {
                            self.connect();
                        }
                    } else if ui.button("Prekini povezavo").clicked() {
                        self.disconnect();
                        self.status = "Povezava prekinjena".into();
                    }
                    ui.label(&self.status);
                });
                if let Some(fp) = self.trust.clone() {
                    ui.group(|ui| {
                        ui.label(
                        "Prva povezava: primerjaj SHA-256 odtis s tistim, ki ga izpiše strežnik.",
                    );
                        ui.add(egui::Label::new(egui::RichText::new(&fp).monospace()).wrap());
                        if ui.button("Zaupam temu strežniku in se poveži").clicked() {
                            self.prefs
                                .pins
                                .insert(self.prefs.address.trim().to_owned(), fp);
                            self.connect();
                        }
                    });
                }
                ui.add_space(12.0);
                ui.separator();
                let room = self.room.load(Ordering::Relaxed);
                ui.horizontal(|ui| {
                    ui.label(format!("Strežnik: {}", ping_label(self.ping)));
                    ui.separator();
                    ui.label(format!(
                        "Klic: {}",
                        if room > 0 {
                            ping_label(self.voice_ping)
                        } else {
                            "—".into()
                        }
                    ));
                    ui.separator();
                    ui.label("Opus · 128 kb/s · mono");
                });
                ui.label(
                    egui::RichText::new(
                        "Ping = povratni čas do strežnika. Osveževanje vsako sekundo.",
                    )
                    .small()
                    .weak(),
                );
                ui.add_space(12.0);
                for r in 1..=4 {
                    let members: Vec<_> =
                        self.peers.iter().filter(|p| p.room == r).cloned().collect();
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            let text = if room == r {
                                format!("Klic {r} (povezan)")
                            } else {
                                format!("Klic {r}")
                            };
                            ui.heading(text);
                            ui.label(format!("{} oseb", members.len()));
                            if ui
                                .add_enabled(
                                    self.id.is_some()
                                        && room != r
                                        && (self.audio.is_some() || self.audio_channels.is_some()),
                                    egui::Button::new("Pridruži se"),
                                )
                                .clicked()
                            {
                                self.join(r);
                            }
                        });
                        if members.is_empty() {
                            ui.label(egui::RichText::new("Ni še nikogar").weak());
                        }
                        for p in members {
                            let speaking = self
                                .speaking
                                .get(&p.id)
                                .is_some_and(|t| t.elapsed() < Duration::from_millis(300));
                            let suffix = if p.muted {
                                "  [mikrofon utišan]"
                            } else if speaking {
                                "  • govori"
                            } else {
                                ""
                            };
                            let label = format!(
                                "{}{}{}",
                                p.name,
                                if Some(p.id) == self.id { " (ti)" } else { "" },
                                suffix
                            );
                            ui.label(egui::RichText::new(label).color(if speaking && !p.muted {
                                egui::Color32::LIGHT_GREEN
                            } else {
                                egui::Color32::LIGHT_GRAY
                            }));
                        }
                    });
                    ui.add_space(8.0);
                }
                ui.horizontal(|ui| {
                    let mut mute = self.muted.load(Ordering::Relaxed);
                    if ui
                        .add_enabled(
                            self.id.is_some(),
                            egui::Checkbox::new(&mut mute, "Utišaj mikrofon"),
                        )
                        .changed()
                    {
                        self.muted.store(mute, Ordering::Relaxed);
                        self.command(Command::Mute(mute));
                    }
                    if ui
                        .add_enabled(room > 0, egui::Button::new("Zapusti klic"))
                        .clicked()
                    {
                        self.join(0);
                    }
                });
                let lobby = self
                    .peers
                    .iter()
                    .filter(|p| p.room == 0)
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                if !lobby.is_empty() {
                    ui.label(format!("V preddverju: {lobby}"));
                }
                ui.add_space(10.0);
                ui.label(
                egui::RichText::new(
                    "Za prvo testiranje uporabi slušalke. Ta različica nima odstranjevanja odmeva.",
                )
                .small()
                .weak(),
            );
            });
        });
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "preferences", &self.prefs);
    }
}
impl Drop for App {
    fn drop(&mut self) {
        self.disconnect();
    }
}
fn ping_label(value: Option<(f64, Instant)>) -> String {
    match value {
        Some((ms, t)) if t.elapsed() < Duration::from_secs(3) => format!("{ms:.1} ms"),
        Some(_) => "ni odziva".into(),
        None => "—".into(),
    }
}
fn main() -> eframe::Result {
    eframe::run_native(
        "ServerKlic",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([650.0, 760.0])
                .with_min_inner_size([560.0, 680.0]),
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
