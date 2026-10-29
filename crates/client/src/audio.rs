use crate::network::{emit, UiEvent, UiKind};
use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serverklic_protocol::*;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub struct AudioEngine {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    _input: cpal::Stream,
    _output: cpal::Stream,
}
impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}
struct Resampler {
    samples: VecDeque<f32>,
    position: f64,
    step: f64,
}
impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Self {
            samples: VecDeque::new(),
            position: 0.0,
            step: from as f64 / to as f64,
        }
    }
    fn push(&mut self, data: &[f32]) -> Vec<f32> {
        self.samples.extend(data);
        let mut out = Vec::new();
        while self.position + 1.0 < self.samples.len() as f64 {
            let i = self.position as usize;
            let frac = (self.position - i as f64) as f32;
            out.push(self.samples[i] * (1.0 - frac) + self.samples[i + 1] * frac);
            self.position += self.step;
        }
        let remove = (self.position.floor() as usize).min(self.samples.len());
        self.samples.drain(..remove);
        self.position -= remove as f64;
        out
    }
    fn clear(&mut self) {
        self.samples.clear();
        self.position = 0.0;
    }
}
fn input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    tx: mpsc::SyncSender<Vec<f32>>,
    room: Arc<AtomicU8>,
    muted: Arc<AtomicBool>,
    err: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = config.channels as usize;
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            if room.load(Ordering::Relaxed) == 0 || muted.load(Ordering::Relaxed) {
                return;
            }
            use cpal::Sample;
            let mono = data
                .chunks_exact(channels)
                .map(|frame| {
                    frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / channels as f32
                })
                .collect();
            let _ = tx.try_send(mono);
        },
        err,
        None,
    )?)
}
fn output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    queue: Arc<Mutex<VecDeque<f32>>>,
    err: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    Ok(device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let mut queue = queue.lock().unwrap();
            for frame in data.chunks_mut(channels) {
                let sample = T::from_sample(queue.pop_front().unwrap_or(0.0));
                for s in frame {
                    *s = sample;
                }
            }
        },
        err,
        None,
    )?)
}
impl AudioEngine {
    pub fn start(
        room: Arc<AtomicU8>,
        muted: Arc<AtomicBool>,
        out: tokio::sync::mpsc::Sender<AudioPacket>,
        incoming: mpsc::Receiver<AudioPacket>,
        ui: mpsc::Sender<UiEvent>,
        session: Arc<AtomicBool>,
    ) -> Result<Self> {
        let host = cpal::default_host();
        let input = host.default_input_device().context("Mikrofon ni najden")?;
        let output = host
            .default_output_device()
            .context("Zvočniki niso najdeni")?;
        let input_cfg = input.default_input_config()?;
        let output_cfg = output.default_output_config()?;
        let input_rate = input_cfg.sample_rate().0;
        let output_rate = output_cfg.sample_rate().0;
        let (tx, rx) = mpsc::sync_channel(8);
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let error_ui = ui.clone();
        let error_session = session.clone();
        let err = move |e| {
            emit(
                &error_ui,
                &error_session,
                UiKind::AudioError(format!("Napaka mikrofona: {e}")),
            )
        };
        let i = match input_cfg.sample_format() {
            cpal::SampleFormat::F32 => input_stream::<f32>(
                &input,
                &input_cfg.into(),
                tx,
                room.clone(),
                muted.clone(),
                err,
            )?,
            cpal::SampleFormat::I16 => input_stream::<i16>(
                &input,
                &input_cfg.into(),
                tx,
                room.clone(),
                muted.clone(),
                err,
            )?,
            cpal::SampleFormat::U16 => input_stream::<u16>(
                &input,
                &input_cfg.into(),
                tx,
                room.clone(),
                muted.clone(),
                err,
            )?,
            other => bail!("Nepodprt format mikrofona: {other:?}"),
        };
        let error_ui = ui.clone();
        let error_session = session.clone();
        let err = move |e| {
            emit(
                &error_ui,
                &error_session,
                UiKind::AudioError(format!("Napaka predvajanja: {e}")),
            )
        };
        let o = match output_cfg.sample_format() {
            cpal::SampleFormat::F32 => {
                output_stream::<f32>(&output, &output_cfg.into(), queue.clone(), err)?
            }
            cpal::SampleFormat::I16 => {
                output_stream::<i16>(&output, &output_cfg.into(), queue.clone(), err)?
            }
            cpal::SampleFormat::U16 => {
                output_stream::<u16>(&output, &output_cfg.into(), queue.clone(), err)?
            }
            other => bail!("Nepodprt format zvočnikov: {other:?}"),
        };
        let mut encoder = opus::Encoder::new(48000, opus::Channels::Mono, opus::Application::Voip)?;
        encoder.set_bitrate(opus::Bitrate::Bits(128000))?;
        encoder.set_vbr(false)?;
        encoder.set_complexity(5)?;
        i.play()?;
        o.play()?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let worker = thread::spawn(move || {
            let mut input_resampler = Resampler::new(input_rate, 48000);
            let mut output_resampler = Resampler::new(48000, output_rate);
            let mut capture = VecDeque::new();
            let mut speakers: BTreeMap<u64, Speaker> = BTreeMap::new();
            let mut seq = 0u32;
            let mut previous_room = 0;
            let mut previous_mute = false;
            let mut deadline = Instant::now();
            while !thread_stop.load(Ordering::Relaxed) && !session.load(Ordering::Relaxed) {
                let r = room.load(Ordering::Relaxed);
                let mute = muted.load(Ordering::Relaxed);
                if r != previous_room || mute != previous_mute {
                    capture.clear();
                    input_resampler.clear();
                    while rx.try_recv().is_ok() {}
                    if r != previous_room {
                        speakers.clear();
                        output_resampler.clear();
                        queue.lock().unwrap().clear();
                    }
                    previous_room = r;
                    previous_mute = mute;
                }
                while let Ok(samples) = rx.try_recv() {
                    if r > 0 && !mute {
                        capture.extend(input_resampler.push(&samples));
                    }
                }
                if capture.len() > FRAME * 3 {
                    capture.drain(..capture.len() - FRAME * 3);
                }
                while capture.len() >= FRAME && r > 0 && !mute {
                    let samples: Vec<f32> = capture.drain(..FRAME).collect();
                    let rms = (samples.iter().map(|x| x * x).sum::<f32>() / FRAME as f32).sqrt();
                    let mut encoded = vec![0u8; 600];
                    match encoder.encode_float(&samples, &mut encoded) {
                        Ok(n) => {
                            encoded.truncate(n);
                            let _ = out.try_send(AudioPacket {
                                sender: 0,
                                room: r,
                                sequence: seq,
                                speaking: rms > 0.008,
                                opus: encoded,
                            });
                            seq = seq.wrapping_add(1);
                        }
                        Err(e) => {
                            emit(&ui, &session, UiKind::AudioError(format!("Opus: {e}")));
                            return;
                        }
                    }
                }
                while let Ok(packet) = incoming.try_recv() {
                    if packet.room != r || r == 0 {
                        continue;
                    }
                    if !speakers.contains_key(&packet.sender) && speakers.len() < MAX_PEERS {
                        if let Ok(s) = Speaker::new(packet.sequence) {
                            speakers.insert(packet.sender, s);
                        }
                    }
                    if let Some(s) = speakers.get_mut(&packet.sender) {
                        s.push(packet);
                    }
                }
                let mut mix = vec![0.0f32; FRAME];
                let now = Instant::now();
                speakers.retain(|_, s| now.duration_since(s.last) < Duration::from_millis(500));
                for speaker in speakers.values_mut() {
                    speaker.mix(&mut mix);
                }
                for x in &mut mix {
                    *x = x.clamp(-1.0, 1.0);
                }
                if r > 0 {
                    let resampled = output_resampler.push(&mix);
                    let mut q = queue.lock().unwrap();
                    // Bound accumulated playback latency if the OS stalls.
                    if q.len() > output_rate as usize / 10 {
                        q.clear();
                    }
                    q.extend(resampled);
                }
                deadline += Duration::from_millis(20);
                if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
                    thread::sleep(wait);
                } else {
                    deadline = Instant::now();
                }
            }
        });
        Ok(Self {
            stop,
            worker: Some(worker),
            _input: i,
            _output: o,
        })
    }
}
struct Speaker {
    decoder: opus::Decoder,
    packets: BTreeMap<u32, Vec<u8>>,
    next: u32,
    ready: Instant,
    last: Instant,
    missed: u32,
}
impl Speaker {
    fn new(seq: u32) -> Result<Self> {
        Ok(Self {
            decoder: opus::Decoder::new(48000, opus::Channels::Mono)?,
            packets: BTreeMap::new(),
            next: seq,
            ready: Instant::now() + Duration::from_millis(60),
            last: Instant::now(),
            missed: 0,
        })
    }
    fn push(&mut self, p: AudioPacket) {
        // Resume cleanly after mute or a network gap; old/out-of-order frames are bounded.
        if self.missed > 5 {
            self.next = p.sequence;
            self.packets.clear();
            self.ready = Instant::now() + Duration::from_millis(60);
            self.missed = 0;
            let _ = self.decoder.reset_state();
        }
        if p.sequence.wrapping_sub(self.next) < 16 {
            self.packets.insert(p.sequence, p.opus);
            self.last = Instant::now();
        }
    }
    fn mix(&mut self, mix: &mut [f32]) {
        if Instant::now() < self.ready {
            return;
        }
        let packet = self.packets.remove(&self.next);
        self.next = self.next.wrapping_add(1);
        if packet.is_some() {
            self.missed = 0;
        } else {
            self.missed += 1;
        }
        if self.missed > 5 {
            return;
        }
        let mut decoded = [0.0f32; FRAME];
        if let Ok(n) =
            self.decoder
                .decode_float(packet.as_deref().unwrap_or(&[]), &mut decoded, false)
        {
            for (dst, src) in mix.iter_mut().zip(&decoded[..n]) {
                *dst += src;
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resampler_preserves_duration_across_irregular_chunks() {
        let mut r = Resampler::new(44100, 48000);
        let mut n = 0;
        for _ in 0..100 {
            n += r.push(&vec![0.25; 441]).len();
        }
        assert!((n as i32 - 48000).abs() <= 2);
    }
    #[test]
    fn codec_128k_and_jitter_reordering() {
        let mut e =
            opus::Encoder::new(48000, opus::Channels::Mono, opus::Application::Voip).unwrap();
        e.set_bitrate(opus::Bitrate::Bits(128000)).unwrap();
        e.set_vbr(false).unwrap();
        let samples: Vec<_> = (0..FRAME)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin() * 0.2)
            .collect();
        let mut bytes = vec![0; 600];
        let n = e.encode_float(&samples, &mut bytes).unwrap();
        assert_eq!(n, 320);
        bytes.truncate(n);
        let mut s = Speaker::new(10).unwrap();
        for sequence in [11, 10, 12] {
            s.push(AudioPacket {
                sender: 1,
                room: 1,
                sequence,
                speaking: true,
                opus: bytes.clone(),
            });
        }
        assert_eq!(s.packets.len(), 3);
        s.ready = Instant::now();
        let mut mix = [0.0; FRAME];
        s.mix(&mut mix);
        assert_eq!(s.next, 11);
        assert!(mix.iter().any(|x| x.abs() > 0.001));
        s.mix(&mut mix);
        s.mix(&mut mix);
        s.mix(&mut mix);
        assert_eq!(s.missed, 1);
    }
}
