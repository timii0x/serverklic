use anyhow::{bail, Result};
use quinn::{Connection, RecvStream};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    DigitallySignedStruct, SignatureScheme,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

pub const VERSION: u32 = 1;
pub const MAX_PEERS: usize = 32;
pub const MAX_CONTROL: usize = 32768;
pub const FRAME: usize = 960; // 20 ms, 48 kHz mono
pub const AUDIO_HEADER: usize = 15;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Peer {
    pub id: u64,
    pub name: String,
    pub room: u8,
    pub muted: bool,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Hello {
    pub version: u32,
    pub name: String,
    pub password: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub enum Command {
    Join(u8),
    Mute(bool),
    Ping(u64),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Event {
    Welcome { id: u64 },
    State(Vec<Peer>),
    Pong(u64),
    Error(String),
}

pub fn valid_name(s: &str) -> bool {
    !s.trim().is_empty() && s.chars().count() <= 32 && !s.chars().any(char::is_control)
}
pub async fn read_json<T: serde::de::DeserializeOwned>(mut stream: RecvStream) -> Result<T> {
    let data =
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(MAX_CONTROL)).await??;
    Ok(serde_json::from_slice(&data)?)
}
pub async fn send_json<T: Serialize>(conn: &Connection, value: &T) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    if data.len() > MAX_CONTROL {
        bail!("Control message too large");
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut s = conn.open_uni().await?;
        s.write_all(&data).await?;
        s.finish()?;
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}
pub fn transport() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(2)));
    t.max_idle_timeout(Some(Duration::from_secs(10).try_into().unwrap()));
    t.max_concurrent_uni_streams(16u32.into());
    t.max_concurrent_bidi_streams(0u32.into());
    t.datagram_receive_buffer_size(Some(128 * 1024));
    t.datagram_send_buffer_size(128 * 1024);
    Arc::new(t)
}
pub fn fingerprint(cert: &[u8]) -> String {
    format!("{:x}", Sha256::digest(cert))
}

// Discovery exposes a fingerprint only. No name, password or audio is sent until
// the user explicitly approves it; subsequent connections enforce the pin.
#[derive(Debug)]
pub struct PinnedVerifier {
    pub expected: Option<String>,
    pub observed: Arc<Mutex<Option<String>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}
impl PinnedVerifier {
    pub fn new(expected: Option<String>, observed: Arc<Mutex<Option<String>>>) -> Arc<Self> {
        Arc::new(Self {
            expected,
            observed,
            provider: Arc::new(rustls::crypto::ring::default_provider()),
        })
    }
}
impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        cert: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint(cert.as_ref());
        *self.observed.lock().unwrap() = Some(fp.clone());
        if self
            .expected
            .as_ref()
            .is_some_and(|expected| expected != &fp)
        {
            return Err(rustls::Error::General(
                "Server certificate changed; connection blocked".into(),
            ));
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
pub fn client_config(
    pin: Option<String>,
    observed: Arc<Mutex<Option<String>>>,
) -> Result<quinn::ClientConfig> {
    let crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(PinnedVerifier::new(pin, observed))
        .with_no_client_auth();
    let mut cfg = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)?,
    ));
    cfg.transport_config(transport());
    Ok(cfg)
}

#[derive(Clone, Debug)]
pub struct AudioPacket {
    pub sender: u64,
    pub room: u8,
    pub sequence: u32,
    pub speaking: bool,
    pub opus: Vec<u8>,
}
impl AudioPacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(AUDIO_HEADER + self.opus.len());
        b.push(1);
        b.extend(self.sender.to_be_bytes());
        b.push(self.room);
        b.extend(self.sequence.to_be_bytes());
        b.push(u8::from(self.speaking));
        b.extend(&self.opus);
        b
    }
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() <= AUDIO_HEADER
            || b.len() > AUDIO_HEADER + 600
            || b[0] != 1
            || !(1..=4).contains(&b[9])
        {
            return None;
        }
        Some(Self {
            sender: u64::from_be_bytes(b[1..9].try_into().ok()?),
            room: b[9],
            sequence: u32::from_be_bytes(b[10..14].try_into().ok()?),
            speaking: b[14] != 0,
            opus: b[15..].to_vec(),
        })
    }
}
pub fn ping_datagram(nonce: u64, room: u8) -> Vec<u8> {
    let mut b = vec![2, room];
    b.extend(nonce.to_be_bytes());
    b
}
pub fn decode_ping(b: &[u8]) -> Option<(u64, u8)> {
    if b.len() != 10 || b[0] != 2 {
        return None;
    }
    Some((u64::from_be_bytes(b[2..10].try_into().ok()?), b[1]))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audio_round_trip_and_reject_bad_input() {
        let p = AudioPacket {
            sender: 9,
            room: 4,
            sequence: u32::MAX,
            speaking: true,
            opus: vec![7; 320],
        };
        let bytes = p.encode();
        let q = AudioPacket::decode(&bytes).unwrap();
        assert_eq!(q.sender, 9);
        assert_eq!(q.opus, p.opus);
        assert_eq!(q.sequence, u32::MAX);
        for n in 0..=15 {
            assert!(AudioPacket::decode(&bytes[..n]).is_none());
        }
        let mut invalid = bytes;
        invalid[9] = 5;
        assert!(AudioPacket::decode(&invalid).is_none());
    }
    #[test]
    fn input_validation() {
        assert!(valid_name("Timi"));
        assert!(!valid_name("  "));
        assert!(!valid_name("a\nb"));
        assert!(!valid_name(&"a".repeat(33)));
        assert_eq!(decode_ping(&ping_datagram(123, 2)), Some((123, 2)));
    }
}
