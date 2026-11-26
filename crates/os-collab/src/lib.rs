// SPDX-License-Identifier: GPL-3.0-or-later
//! Direct, invitation-pinned TLS sessions. No public service, audio or UI dependency.
//! Every blocking call belongs on a background thread. Drop never joins a worker.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

pub mod assets;

pub type Document = BTreeMap<String, Value>;
pub type Result<T> = std::result::Result<T, String>;
const MAX_FRAME: usize = 64 * 1024 * 1024;
pub const MAX_PACKAGE: u64 = 512 * 1024 * 1024;
const MAX_DOCUMENT: usize = 24 * 1024 * 1024;
const CHUNK: usize = 128 * 1024;
const PROTOCOL: u32 = 3;
const IROH_ALPN: &[u8] = b"mlstudio-collab/3";

pub fn random_id() -> Result<String> {
    let mut b = [0; 32];
    getrandom::fill(&mut b).map_err(|e| e.to_string())?;
    Ok(B64.encode(b))
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Change {
    pub key: String,
    #[serde(with = "optional_value")]
    pub before: Option<Value>,
    #[serde(with = "optional_value")]
    pub after: Option<Value>,
}
// JSON null is a real project value, distinct from a deleted/missing field.
// Ordinary Option<Value> decoding collapses these two states and corrupts new objects.
mod optional_value {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        v: &Option<Value>,
        s: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        v.as_ref().map(|v| [v]).serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<Option<Value>, D::Error> {
        Ok(Option::<[Value; 1]>::deserialize(d)?.map(|[v]| v))
    }
}
pub fn diff(before: &Document, after: &Document) -> Vec<Change> {
    let mut keys: Vec<_> = before.keys().chain(after.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter_map(|k| {
            let (a, b) = (before.get(k), after.get(k));
            (a != b).then(|| Change {
                key: k.clone(),
                before: a.cloned(),
                after: b.cloned(),
            })
        })
        .collect()
}
/// Atomic compare-and-set. Conflicts never partially apply or overwrite other work.
pub fn apply(doc: &mut Document, changes: &[Change]) -> Result<()> {
    if changes.len() > 65536 {
        return Err("Too many changes".into());
    }
    let mut keys = std::collections::BTreeSet::new();
    for c in changes {
        if c.key.is_empty()
            || c.key.len() > 160
            || !keys.insert(&c.key)
            || doc.get(&c.key) != c.before.as_ref()
        {
            return Err(format!("Conflicting edit: {}", c.key));
        }
    }
    for c in changes {
        if let Some(v) = &c.after {
            doc.insert(c.key.clone(), v.clone());
        } else {
            doc.remove(&c.key);
        }
    }
    Ok(())
}
pub fn inverse(changes: &[Change]) -> Vec<Change> {
    changes
        .iter()
        .map(|c| Change {
            key: c.key.clone(),
            before: c.after.clone(),
            after: c.before.clone(),
        })
        .collect()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Invitation {
    pub address: String,
    certificate: String,
    secret: String,
    protocol: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p2p: Option<iroh::EndpointAddr>,
}
impl Invitation {
    pub fn encode(&self) -> Result<String> {
        Ok(format!(
            "mlstudio-session:{}",
            B64.encode(serde_json::to_vec(self).map_err(err)?)
        ))
    }
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > 8192 {
            return Err("Invitation is too long".into());
        }
        let bytes = B64
            .decode(
                text.trim()
                    .strip_prefix("mlstudio-session:")
                    .ok_or("Paste an JL Studio session invitation")?,
            )
            .map_err(err)?;
        let i: Self = serde_json::from_slice(&bytes).map_err(err)?;
        if i.protocol != PROTOCOL
            || i.secret.len() != 43
            || i.address.len() > 260
            || i.certificate.len() > 4096
        {
            return Err("Unsupported or invalid invitation".into());
        }
        Ok(i)
    }
    pub fn with_address(&self, address: String) -> Self {
        Self {
            address,
            ..self.clone()
        }
    }
    pub fn with_local_tcp(&self, address: String) -> Self {
        Self {
            address,
            p2p: None,
            ..self.clone()
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub seq: u64,
    pub changes: Vec<Change>,
}
#[derive(Serialize, Deserialize)]
enum Request {
    Hello {
        protocol: u32,
        secret: String,
        peer: String,
        name: String,
    },
    Poll {
        since: u64,
        proposal: Option<Proposal>,
        playback: Option<Playback>,
    },
    Chunk {
        offset: u64,
    },
    AudioInfo {
        id: String,
    },
    AudioPut {
        id: String,
        total: u64,
        offset: u64,
        data: String,
    },
    AudioGet {
        id: String,
        offset: u64,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Playback {
    pub playing: bool,
    pub song: bool,
    pub tick: f64,
    pub tempo: f64,
    pub ppq: u16,
    pub pattern: u16,
}
impl Playback {
    pub fn advance(&mut self, seconds: f64) {
        if self.playing {
            self.tick += seconds * self.tempo / 60. * f64::from(self.ppq);
        }
    }
    fn valid(&self) -> bool {
        self.tick.is_finite()
            && self.tick >= 0.
            && self.tick < u32::MAX as f64
            && self.tempo.is_finite()
            && (10. ..=522.).contains(&self.tempo)
            && self.ppq > 0
            && self.ppq <= 9600
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Update {
    pub revision: u64,
    pub changes: Vec<Change>,
    pub accepted: Option<bool>,
    pub rejection: Option<String>,
    pub peers: Vec<String>,
    pub playback: Option<Playback>,
}
#[derive(Serialize, Deserialize)]
enum Reply {
    Welcome {
        document: Document,
        revision: u64,
        size: u64,
        hash: String,
    },
    Update(Update),
    Chunk(String),
    AudioInfo {
        size: u64,
        complete: bool,
    },
    Error(String),
}
struct Peer {
    seq: u64,
    accepted: bool,
    rejection: Option<String>,
    name: String,
    seen: std::time::Instant,
}
struct Session {
    asset_root: PathBuf,
    leader: Option<String>,
    playback: Option<(Playback, std::time::Instant)>,
    doc: Document,
    revision: u64,
    versions: BTreeMap<String, u64>,
    peers: BTreeMap<String, Peer>,
    validate: fn(&Document) -> Result<()>,
}
impl Session {
    fn poll(&mut self, peer: &str, since: u64, proposal: Option<Proposal>) -> Result<Update> {
        if since > self.revision {
            return Err("Session revision mismatch".into());
        }
        let mut accepted = None;
        if let Some(p) = proposal {
            let last = self.peers.get(peer).ok_or("Unknown participant")?;
            if p.seq == last.seq {
                accepted = Some(last.accepted);
            } else if p.seq != last.seq + 1 {
                return Err("Edit sequence mismatch".into());
            } else {
                let mut candidate = self.doc.clone();
                let checked = if p.changes.iter().any(|c| c.key == "schema")
                    || self.versions.len() + p.changes.len() > 131072
                {
                    Err("Unsupported session schema or size".into())
                } else {
                    apply(&mut candidate, &p.changes)
                        .and_then(|_| bounded(&candidate))
                        .and_then(|_| (self.validate)(&candidate))
                        .and_then(|_| assets::verify_references(&candidate, &self.asset_root))
                };
                let ok = checked.is_ok();
                let reason = checked.err();
                if ok && !p.changes.is_empty() {
                    self.revision += 1;
                    for c in &p.changes {
                        self.versions.insert(c.key.clone(), self.revision);
                    }
                    self.doc = candidate;
                }
                let last = self.peers.get_mut(peer).unwrap();
                last.seq = p.seq;
                last.accepted = ok;
                last.rejection = reason;
                accepted = Some(ok);
            }
        }
        self.peers.get_mut(peer).ok_or("Unknown participant")?.seen = std::time::Instant::now();
        Ok(Update {
            revision: self.revision,
            playback: self.playback.as_ref().map(|(p, t)| {
                let mut p = p.clone();
                p.advance(t.elapsed().as_secs_f64());
                p
            }),
            changes: self
                .versions
                .iter()
                .filter(|(_, r)| **r > since)
                .map(|(k, _)| Change {
                    key: k.clone(),
                    before: None,
                    after: self.doc.get(k).cloned(),
                })
                .collect(),
            accepted,
            rejection: if accepted == Some(false) {
                self.peers.get(peer).and_then(|p| p.rejection.clone())
            } else {
                None
            },
            peers: self
                .peers
                .values()
                .filter(|p| p.seen.elapsed() < Duration::from_secs(12))
                .map(|p| p.name.clone())
                .collect(),
        })
    }
}
fn bounded(d: &Document) -> Result<()> {
    if d.len() > 65536 || serde_json::to_vec(d).map_err(err)?.len() > MAX_DOCUMENT {
        Err("Shared arrangement is too large".into())
    } else {
        Ok(())
    }
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn send<T: Serialize>(s: &mut impl Write, v: &T) -> Result<()> {
    let bytes = serde_json::to_vec(v).map_err(err)?;
    if bytes.len() > MAX_FRAME {
        return Err("Session message is too large".into());
    }
    s.write_all(&(bytes.len() as u32).to_be_bytes())
        .map_err(err)?;
    s.write_all(&bytes).map_err(err)?;
    s.flush().map_err(err)
}
fn recv<T: serde::de::DeserializeOwned>(s: &mut impl Read) -> Result<T> {
    let mut size = [0; 4];
    s.read_exact(&mut size).map_err(err)?;
    let size = u32::from_be_bytes(size) as usize;
    if size > MAX_FRAME {
        return Err("Session message is too large".into());
    }
    let mut bytes = vec![0; size];
    s.read_exact(&mut bytes).map_err(err)?;
    serde_json::from_slice(&bytes).map_err(err)
}
fn hash_file(path: &Path) -> Result<String> {
    let mut f = File::open(path).map_err(err)?;
    let mut hash = Sha256::new();
    let mut b = [0; 65536];
    loop {
        let n = f.read(&mut b).map_err(err)?;
        if n == 0 {
            break;
        }
        hash.update(&b[..n]);
    }
    Ok(B64.encode(hash.finalize()))
}
fn configure(s: &TcpStream) -> Result<()> {
    s.set_nonblocking(false).map_err(err)?;
    s.set_nodelay(true).map_err(err)?;
    s.set_read_timeout(Some(Duration::from_secs(4)))
        .map_err(err)?;
    s.set_write_timeout(Some(Duration::from_secs(4)))
        .map_err(err)
}
struct IrohStream {
    rt: Arc<tokio::runtime::Runtime>,
    ep: Option<iroh::Endpoint>,
    conn: iroh::endpoint::Connection,
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
}
impl Drop for IrohStream {
    fn drop(&mut self) {
        self.conn.close(0u32.into(), b"bye");
        if let Some(ep) = self.ep.take() {
            let _ = self.rt.block_on(async move {
                let _ = tokio::time::timeout(Duration::from_millis(300), ep.close()).await;
            });
        }
    }
}
impl Read for IrohStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.rt.block_on(async {
            match tokio::time::timeout(
                Duration::from_secs(10),
                tokio::io::AsyncReadExt::read(&mut self.recv, buf),
            )
            .await
            {
                Ok(res) => res,
                Err(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "P2P read timed out",
                )),
            }
        })
    }
}
impl Write for IrohStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.rt.block_on(async {
            match tokio::time::timeout(
                Duration::from_secs(10),
                tokio::io::AsyncWriteExt::write(&mut self.send, buf),
            )
            .await
            {
                Ok(res) => res,
                Err(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "P2P write timed out",
                )),
            }
        })
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.rt.block_on(async {
            match tokio::time::timeout(
                Duration::from_secs(10),
                tokio::io::AsyncWriteExt::flush(&mut self.send),
            )
            .await
            {
                Ok(res) => res,
                Err(_) => Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "P2P flush timed out",
                )),
            }
        })
    }
}
pub struct Host {
    pub invitation: Invitation,
    pub port: u16,
    stop: Arc<AtomicBool>,
    iroh: Option<(Arc<tokio::runtime::Runtime>, iroh::Endpoint)>,
}
impl Drop for Host {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some((rt, ep)) = self.iroh.take() {
            let _ = rt.block_on(async move {
                let _ = tokio::time::timeout(Duration::from_millis(300), ep.close()).await;
            });
        }
    }
}
impl Host {
    /// Binds only after an explicit Host action. Invitation grants project access.
    pub fn start(
        bind: SocketAddr,
        address: String,
        package: PathBuf,
        doc: Document,
        validate: fn(&Document) -> Result<()>,
    ) -> Result<Self> {
        Self::start_inner(bind, address, package, doc, validate, false)
    }
    /// Starts a P2P-enabled session using iroh (QUIC hole punching + relay) plus a local
    /// loopback TLS listener for the host's own client.
    pub fn start_p2p(
        package: PathBuf,
        doc: Document,
        validate: fn(&Document) -> Result<()>,
    ) -> Result<Self> {
        Self::start_inner(
            "127.0.0.1:0".parse().unwrap(),
            String::new(),
            package,
            doc,
            validate,
            true,
        )
    }
    fn start_inner(
        bind: SocketAddr,
        address: String,
        package: PathBuf,
        doc: Document,
        validate: fn(&Document) -> Result<()>,
        enable_p2p: bool,
    ) -> Result<Self> {
        bounded(&doc)?;
        validate(&doc)?;
        let asset_root = package
            .parent()
            .ok_or("Missing session folder")?
            .join("assets");
        assets::verify_references(&doc, &asset_root)?;
        let size = std::fs::metadata(&package).map_err(err)?.len();
        if size > MAX_PACKAGE {
            return Err("Project package exceeds 512 MB".into());
        }
        let hash = hash_file(&package)?;
        let generated =
            rcgen::generate_simple_self_signed(vec!["mlstudio-session".into()]).map_err(err)?;
        let cert = generated.cert.der().clone();
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(generated.key_pair.serialize_der());
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(err)?
            .with_no_client_auth()
            .with_single_cert(vec![cert.clone()], key.into())
            .map_err(err)?;
        let config = Arc::new(config);
        let secret = random_id()?;
        let listener = TcpListener::bind(bind).map_err(err)?;
        let port = listener.local_addr().map_err(err)?.port();
        listener.set_nonblocking(true).map_err(err)?;
        let stop = Arc::new(AtomicBool::new(false));
        let halted = stop.clone();
        let session = Arc::new(Mutex::new(Session {
            doc,
            asset_root,
            leader: None,
            playback: None,
            revision: 0,
            versions: BTreeMap::new(),
            peers: BTreeMap::new(),
            validate,
        }));
        let audio = Arc::new(Mutex::new(assets::Store::new(
            package
                .parent()
                .ok_or("Missing session folder")?
                .join("assets"),
        )?));
        let count = Arc::new(AtomicUsize::new(0));
        let token = secret.clone();
        let (tcp_session, tcp_audio, tcp_count, tcp_pkg, tcp_hash) = (
            session.clone(),
            audio.clone(),
            count.clone(),
            package.clone(),
            hash.clone(),
        );
        std::thread::Builder::new()
            .name("session-listener".into())
            .spawn(move || {
                while !halted.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((tcp, _)) => {
                            if tcp_count.load(Ordering::Relaxed) >= 12 {
                                continue;
                            }
                            tcp_count.fetch_add(1, Ordering::Relaxed);
                            let audio = tcp_audio.clone();
                            let (config, session, stop, count, secret, path, hash) = (
                                config.clone(),
                                tcp_session.clone(),
                                halted.clone(),
                                tcp_count.clone(),
                                token.clone(),
                                tcp_pkg.clone(),
                                tcp_hash.clone(),
                            );
                            std::thread::spawn(move || {
                                let result = serve(
                                    tcp, config, session, stop, &secret, &path, size, &hash, audio,
                                );
                                #[cfg(test)]
                                if let Err(e) = &result {
                                    eprintln!("session test: {e}");
                                }
                                let _ = result;
                                count.fetch_sub(1, Ordering::Relaxed);
                            });
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(40))
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(err)?;

        let mut iroh_state = None;
        let mut p2p_addr = None;
        let mut final_address = address;
        if enable_p2p {
            let rt = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .thread_name("collab-iroh-host")
                    .build()
                    .map_err(err)?,
            );
            let ep = rt.block_on(async {
                let ep = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                    .alpns(vec![IROH_ALPN.to_vec()])
                    .bind()
                    .await
                    .map_err(err)?;
                let _ = tokio::time::timeout(Duration::from_secs(4), ep.online()).await;
                Ok::<_, String>(ep)
            })?;
            let addr = ep.addr();
            final_address = format!("p2p:{}", addr.id.fmt_short());
            p2p_addr = Some(addr);

            let accept_ep = ep.clone();
            let accept_rt = rt.clone();
            let halted = stop.clone();
            let token = secret.clone();
            rt.spawn(async move {
                while !halted.load(Ordering::Acquire) {
                    let incoming = tokio::select! {
                        inc = accept_ep.accept() => inc,
                        _ = tokio::time::sleep(Duration::from_millis(200)) => continue,
                    };
                    let Some(incoming) = incoming else { break };
                    if count.load(Ordering::Relaxed) >= 12 {
                        continue;
                    }
                    let rt_stream = accept_rt.clone();
                    let session = session.clone();
                    let stop = halted.clone();
                    let count = count.clone();
                    let secret = token.clone();
                    let path = package.clone();
                    let hash = hash.clone();
                    let audio = audio.clone();
                    tokio::spawn(async move {
                        let Ok(conn) = incoming.await else { return };
                        let Ok((send, recv)) = conn.accept_bi().await else {
                            return;
                        };
                        count.fetch_add(1, Ordering::Relaxed);
                        std::thread::spawn(move || {
                            let stream = IrohStream {
                                rt: rt_stream,
                                ep: None,
                                conn,
                                send,
                                recv,
                            };
                            let result = serve_stream(
                                stream, session, stop, &secret, &path, size, &hash, audio,
                            );
                            #[cfg(test)]
                            if let Err(e) = &result {
                                eprintln!("p2p session test: {e}");
                            }
                            let _ = result;
                            count.fetch_sub(1, Ordering::Relaxed);
                        });
                    });
                }
            });
            iroh_state = Some((rt, ep));
        }

        Ok(Self {
            invitation: Invitation {
                address: final_address,
                certificate: B64.encode(cert.as_ref()),
                secret,
                protocol: PROTOCOL,
                p2p: p2p_addr,
            },
            port,
            stop,
            iroh: iroh_state,
        })
    }
}
fn serve(
    tcp: TcpStream,
    config: Arc<rustls::ServerConfig>,
    session: Arc<Mutex<Session>>,
    stop: Arc<AtomicBool>,
    secret: &str,
    path: &Path,
    size: u64,
    hash: &str,
    audio: Arc<Mutex<assets::Store>>,
) -> Result<()> {
    configure(&tcp)?;
    let conn = rustls::ServerConnection::new(config).map_err(err)?;
    let io = rustls::StreamOwned::new(conn, tcp);
    serve_stream(io, session, stop, secret, path, size, hash, audio)
}
fn serve_stream<S: Read + Write>(
    mut io: S,
    session: Arc<Mutex<Session>>,
    stop: Arc<AtomicBool>,
    secret: &str,
    path: &Path,
    size: u64,
    hash: &str,
    audio: Arc<Mutex<assets::Store>>,
) -> Result<()> {
    let Request::Hello {
        protocol,
        secret: token,
        peer,
        name,
    } = recv(&mut io)?
    else {
        return Err("Expected invitation".into());
    };
    let matches = token.len() == secret.len()
        && token
            .bytes()
            .zip(secret.bytes())
            .fold(0u8, |x, (a, b)| x | (a ^ b))
            == 0;
    if protocol != PROTOCOL || !matches || peer.len() != 43 || name.is_empty() || name.len() > 64 {
        return Err("Invalid invitation".into());
    }
    {
        let mut s = session.lock().map_err(err)?;
        if !s.peers.contains_key(&peer) && s.peers.len() >= 8 {
            return Err("Session participant limit reached".into());
        }
        if s.leader.is_none() {
            s.leader = Some(peer.clone());
        }
        s.peers.entry(peer.clone()).or_insert(Peer {
            seq: 0,
            accepted: true,
            rejection: None,
            name,
            seen: std::time::Instant::now(),
        });
        // Clone while locked; network writes never hold the session mutex.
        let welcome = Reply::Welcome {
            document: s.doc.clone(),
            revision: s.revision,
            size,
            hash: hash.into(),
        };
        drop(s);
        send(&mut io, &welcome)?;
    }
    let mut file = File::open(path).map_err(err)?;
    while !stop.load(Ordering::Acquire) {
        let req: Request = recv(&mut io)?;
        let reply = match req {
            Request::Poll {
                since,
                proposal,
                playback,
            } => {
                let result = {
                    let mut s = session.lock().map_err(err)?;
                    if s.leader.as_ref() == Some(&peer) {
                        if let Some(p) = playback {
                            if p.valid() {
                                s.playback = Some((p, std::time::Instant::now()));
                            }
                        }
                    }
                    s.poll(&peer, since, proposal)
                };
                match result {
                    Ok(u) => Reply::Update(u),
                    Err(e) => Reply::Error(e),
                }
            }
            Request::Chunk { offset } => {
                if offset > size {
                    return Err("Invalid download offset".into());
                }
                file.seek(SeekFrom::Start(offset)).map_err(err)?;
                let mut b = vec![0; CHUNK.min((size - offset) as usize)];
                file.read_exact(&mut b).map_err(err)?;
                Reply::Chunk(B64.encode(b))
            }
            Request::AudioInfo { id } => match audio.lock().map_err(err)?.info(&id) {
                Ok((size, complete)) => Reply::AudioInfo { size, complete },
                Err(e) => Reply::Error(e),
            },
            Request::AudioPut {
                id,
                total,
                offset,
                data,
            } => {
                let data = B64.decode(data).map_err(err)?;
                match audio.lock().map_err(err)?.write(&id, total, offset, &data) {
                    Ok((size, complete)) => Reply::AudioInfo { size, complete },
                    Err(e) => Reply::Error(e),
                }
            }
            Request::AudioGet { id, offset } => {
                match audio.lock().map_err(err)?.read(&id, offset) {
                    Ok(b) => Reply::Chunk(B64.encode(b)),
                    Err(e) => Reply::Error(e),
                }
            }
            _ => return Err("Unexpected message".into()),
        };
        send(&mut io, &reply)?;
    }
    Ok(())
}
enum Transport {
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
    Iroh(Box<IrohStream>),
}
impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Tls(s) => s.read(buf),
            Self::Iroh(s) => s.read(buf),
        }
    }
}
impl Write for Transport {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Tls(s) => s.write(buf),
            Self::Iroh(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Tls(s) => s.flush(),
            Self::Iroh(s) => s.flush(),
        }
    }
}
pub struct Client {
    io: Transport,
    pub document: Document,
    pub revision: u64,
    size: u64,
    hash: String,
}
impl Client {
    pub fn connect(i: &Invitation, peer: &str, name: &str) -> Result<Self> {
        let mut io = if let Some(p2p_addr) = &i.p2p {
            let rt = Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .thread_name("collab-iroh-client")
                    .build()
                    .map_err(err)?,
            );
            let (ep, conn, send, recv) = rt.block_on(async {
                let ep = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                    .bind()
                    .await
                    .map_err(err)?;
                let conn = tokio::time::timeout(
                    Duration::from_secs(12),
                    ep.connect(p2p_addr.clone(), IROH_ALPN),
                )
                .await
                .map_err(|_| "Cannot reach P2P host. Check your internet connection.")?
                .map_err(err)?;
                let (send, recv) = conn.open_bi().await.map_err(err)?;
                Ok::<_, String>((ep, conn, send, recv))
            })?;
            Transport::Iroh(Box::new(IrohStream {
                rt,
                ep: Some(ep),
                conn,
                send,
                recv,
            }))
        } else {
            let cert =
                rustls::pki_types::CertificateDer::from(B64.decode(&i.certificate).map_err(err)?);
            let mut roots = rustls::RootCertStore::empty();
            roots.add(cert).map_err(err)?;
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(err)?
            .with_root_certificates(roots)
            .with_no_client_auth();
            let mut connected = None;
            for addr in i.address.to_socket_addrs().map_err(err)?.take(4) {
                if let Ok(s) = TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
                    connected = Some(s);
                    break;
                }
            }
            let tcp = connected
                .ok_or("Cannot reach host. Check the address, firewall and port forwarding.")?;
            configure(&tcp)?;
            let conn = rustls::ClientConnection::new(
                Arc::new(config),
                rustls::pki_types::ServerName::try_from("mlstudio-session").unwrap(),
            )
            .map_err(err)?;
            Transport::Tls(Box::new(rustls::StreamOwned::new(conn, tcp)))
        };
        send(
            &mut io,
            &Request::Hello {
                protocol: PROTOCOL,
                secret: i.secret.clone(),
                peer: peer.into(),
                name: name.chars().take(32).collect(),
            },
        )?;
        match recv(&mut io)? {
            Reply::Welcome {
                document,
                revision,
                size,
                hash,
            } => {
                bounded(&document)?;
                if size > MAX_PACKAGE {
                    return Err("Shared package is too large".into());
                }
                Ok(Self {
                    io,
                    document,
                    revision,
                    size,
                    hash,
                })
            }
            _ => Err("Host did not accept the invitation".into()),
        }
    }
    pub fn poll(&mut self, proposal: Option<Proposal>) -> Result<Update> {
        self.poll_playback(proposal, None)
    }
    pub fn poll_playback(
        &mut self,
        proposal: Option<Proposal>,
        playback: Option<Playback>,
    ) -> Result<Update> {
        let started = std::time::Instant::now();
        send(
            &mut self.io,
            &Request::Poll {
                since: self.revision,
                proposal,
                playback,
            },
        )?;
        match recv(&mut self.io)? {
            Reply::Update(mut u) => {
                if let Some(p) = &mut u.playback {
                    p.advance(started.elapsed().as_secs_f64() / 2.);
                }
                for c in &u.changes {
                    let (k, v) = (&c.key, &c.after);
                    if let Some(v) = v {
                        self.document.insert(k.clone(), v.clone());
                    } else {
                        self.document.remove(k);
                    }
                }
                if !u.changes.is_empty() {
                    bounded(&self.document)?;
                }
                self.revision = u.revision;
                Ok(u)
            }
            Reply::Error(e) => Err(e),
            _ => Err("Invalid host response".into()),
        }
    }
    fn audio_info(&mut self, id: &str) -> Result<(u64, bool)> {
        send(&mut self.io, &Request::AudioInfo { id: id.into() })?;
        match recv(&mut self.io)? {
            Reply::AudioInfo { size, complete } => Ok((size, complete)),
            Reply::Error(e) => Err(e),
            _ => Err("Invalid audio response".into()),
        }
    }
    pub fn upload_audio(
        &mut self,
        id: &str,
        store: &assets::Store,
        stop: &AtomicBool,
    ) -> Result<()> {
        let (total, ready) = store.info(id)?;
        if !ready {
            return Err("Audio is incomplete".into());
        }
        let (mut offset, complete) = self.audio_info(id)?;
        if complete {
            return Ok(());
        }
        loop {
            if stop.load(Ordering::Acquire) {
                return Err("Cancelled".into());
            }
            let data = store.read(id, offset)?;
            send(
                &mut self.io,
                &Request::AudioPut {
                    id: id.into(),
                    total,
                    offset,
                    data: B64.encode(data),
                },
            )?;
            match recv(&mut self.io)? {
                Reply::AudioInfo { size, complete } => {
                    if (size <= offset && !complete) || size > total {
                        return Err("Invalid upload offset".into());
                    }
                    offset = size;
                    if complete {
                        break;
                    }
                }
                Reply::Error(e) => return Err(e),
                _ => return Err("Invalid upload response".into()),
            }
        }
        Ok(())
    }
    pub fn download_audio(
        &mut self,
        id: &str,
        store: &assets::Store,
        stop: &AtomicBool,
    ) -> Result<()> {
        let (mut offset, complete) = store.info(id)?;
        if complete {
            return Ok(());
        }
        let (total, ready) = self.audio_info(id)?;
        if !ready || total > MAX_PACKAGE || offset > total {
            return Err("Shared audio is incomplete".into());
        }
        loop {
            if stop.load(Ordering::Acquire) {
                return Err("Cancelled".into());
            }
            send(
                &mut self.io,
                &Request::AudioGet {
                    id: id.into(),
                    offset,
                },
            )?;
            let data = match recv(&mut self.io)? {
                Reply::Chunk(b) => B64.decode(b).map_err(err)?,
                Reply::Error(e) => return Err(e),
                _ => return Err("Invalid audio response".into()),
            };
            let (next, done) = store.write(id, total, offset, &data)?;
            if next == offset && !done {
                return Err("Empty audio chunk".into());
            }
            offset = next;
            if done {
                break;
            }
        }
        Ok(())
    }
    /// Package is written only to the caller's fresh session directory; never an arbitrary remote path.
    pub fn download(&mut self, path: &Path, stop: &AtomicBool) -> Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(err)?;
        let mut offset = f.metadata().map_err(err)?.len();
        if offset > self.size {
            return Err("Partial project is larger than the host package".into());
        }
        while offset < self.size {
            if stop.load(Ordering::Acquire) {
                return Err("Cancelled".into());
            }
            send(&mut self.io, &Request::Chunk { offset })?;
            let Reply::Chunk(encoded) = recv(&mut self.io)? else {
                return Err("Invalid package data".into());
            };
            let data = B64.decode(encoded).map_err(err)?;
            if data.is_empty() || data.len() > CHUNK || offset + data.len() as u64 > self.size {
                return Err("Invalid package size".into());
            }
            f.write_all(&data).map_err(err)?;
            offset += data.len() as u64;
        }
        f.sync_all().map_err(err)?;
        drop(f);
        if hash_file(path)? != self.hash {
            let _ = std::fs::remove_file(path);
            return Err("Project download failed its integrity check; retrying a fresh copy".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn valid(_: &Document) -> Result<()> {
        Ok(())
    }
    #[test]
    fn null_and_absence_remain_distinct_over_the_wire() {
        let before = Document::from([("keep".into(), Value::Null), ("remove".into(), Value::Null)]);
        let after = Document::from([("keep".into(), json!(3)), ("new".into(), Value::Null)]);
        let changes = diff(&before, &after);
        let wire: Vec<Change> =
            serde_json::from_slice(&serde_json::to_vec(&changes).unwrap()).unwrap();
        assert_eq!(changes, wire);
        let mut got = before;
        apply(&mut got, &wire).unwrap();
        assert_eq!(got, after);
        let update = Update {
            revision: 1,
            changes: wire,
            accepted: Some(true),
            rejection: None,
            peers: vec![],
            playback: None,
        };
        let wire: Update = serde_json::from_slice(&serde_json::to_vec(&update).unwrap()).unwrap();
        assert_eq!(wire.changes, update.changes);
    }
    #[test]
    fn atomic_conflicts_and_selective_undo() {
        let mut d = Document::from([("p1".into(), json!(1)), ("p2".into(), json!(2))]);
        let mine = vec![Change {
            key: "p1".into(),
            before: Some(json!(1)),
            after: Some(json!(3)),
        }];
        apply(&mut d, &mine).unwrap();
        d.insert("p2".into(), json!(4));
        apply(&mut d, &inverse(&mine)).unwrap();
        assert_eq!(d["p2"], json!(4));
        let before = d.clone();
        assert!(
            apply(
                &mut d,
                &[
                    mine[0].clone(),
                    Change {
                        key: "p2".into(),
                        before: Some(json!(2)),
                        after: None
                    }
                ]
            )
            .is_err()
        );
        assert_eq!(d, before);
    }
    #[test]
    fn encrypted_two_clients_reconnect_dedup_and_package() {
        let root = std::env::temp_dir().join(format!("ml-session-test-{}", random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let package = root.join("host.zip");
        std::fs::write(&package, b"package contents").unwrap();
        let host = Host::start(
            "127.0.0.1:0".parse().unwrap(),
            "".into(),
            package,
            Document::new(),
            valid,
        )
        .unwrap();
        let invite = host
            .invitation
            .with_address(format!("127.0.0.1:{}", host.port));
        let a = random_id().unwrap();
        let b = random_id().unwrap();
        let mut one = Client::connect(
            &Invitation::parse(&invite.encode().unwrap()).unwrap(),
            &a,
            "One",
        )
        .unwrap();
        let mut two = Client::connect(&invite, &b, "Two").unwrap();
        two.download(&root.join("received.zip"), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            std::fs::read(root.join("received.zip")).unwrap(),
            b"package contents"
        );
        let p = Proposal {
            seq: 1,
            changes: vec![Change {
                key: "pattern/1".into(),
                before: None,
                after: Some(json!([60, 64, 67])),
            }],
        };
        assert_eq!(one.poll(Some(p.clone())).unwrap().accepted, Some(true));
        assert_eq!(two.poll(None).unwrap().revision, 1);
        drop(one);
        let mut one = Client::connect(&invite, &a, "One").unwrap();
        assert_eq!(one.poll(Some(p)).unwrap().revision, 1);
        let bad = Proposal {
            seq: 1,
            changes: vec![Change {
                key: "pattern/1".into(),
                before: None,
                after: Some(json!([72])),
            }],
        };
        assert_eq!(two.poll(Some(bad)).unwrap().accepted, Some(false));
        let wrong = Invitation {
            secret: random_id().unwrap(),
            ..invite.clone()
        };
        assert!(Client::connect(&wrong, &random_id().unwrap(), "Intruder").is_err());
        drop(host);
        drop(one);
        drop(two);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn invitation_backwards_compatibility_and_local_tcp_strip() {
        let legacy_json = json!({
            "address": "192.168.1.50:32190",
            "certificate": "AQID",
            "secret": random_id().unwrap(),
            "protocol": PROTOCOL,
        });
        let encoded = format!(
            "mlstudio-session:{}",
            B64.encode(serde_json::to_vec(&legacy_json).unwrap())
        );
        let parsed = Invitation::parse(&encoded).unwrap();
        assert_eq!(parsed.address, "192.168.1.50:32190");
        assert!(parsed.p2p.is_none());
    }
    #[test]
    fn p2p_iroh_two_clients_sync_package_audio_and_reconnect() {
        let root = std::env::temp_dir().join(format!("ml-p2p-test-{}", random_id().unwrap()));
        std::fs::create_dir_all(root.join("host_dir")).unwrap();
        std::fs::create_dir_all(root.join("peer1")).unwrap();
        std::fs::create_dir_all(root.join("peer2")).unwrap();

        let package = root.join("host_dir").join("host.zip");
        std::fs::write(&package, b"p2p package contents 123456").unwrap();

        let host = Host::start_p2p(package, Document::new(), valid).unwrap();
        assert!(host.invitation.p2p.is_some());
        assert!(host.invitation.address.starts_with("p2p:"));

        // Encode & parse invitation just like copy/paste between users.
        let encoded = host.invitation.encode().unwrap();
        let invite = Invitation::parse(&encoded).unwrap();
        assert!(invite.p2p.is_some());

        // Local loopback invite strips p2p so host's own client uses 127.0.0.1.
        let local_invite = host
            .invitation
            .with_local_tcp(format!("127.0.0.1:{}", host.port));
        assert!(local_invite.p2p.is_none());

        let peer_local_id = random_id().unwrap();
        let mut local_client = Client::connect(&local_invite, &peer_local_id, "HostLocal").unwrap();

        // Two remote clients connect via iroh P2P.
        let peer1_id = random_id().unwrap();
        let peer2_id = random_id().unwrap();
        let mut one = Client::connect(&invite, &peer1_id, "RemoteOne").unwrap();
        let mut two = Client::connect(&invite, &peer2_id, "RemoteTwo").unwrap();

        // Download package over P2P and verify integrity.
        two.download(&root.join("peer2").join("received.zip"), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            std::fs::read(root.join("peer2").join("received.zip")).unwrap(),
            b"p2p package contents 123456"
        );

        // Sync edits & playback across P2P and local loopback.
        let proposal = Proposal {
            seq: 1,
            changes: vec![Change {
                key: "pattern/1".into(),
                before: None,
                after: Some(json!([60, 64, 67])),
            }],
        };
        let pb = Playback {
            playing: true,
            song: true,
            tick: 192.0,
            tempo: 145.0,
            ppq: 96,
            pattern: 1,
        };
        let u1 = local_client
            .poll_playback(Some(proposal.clone()), Some(pb))
            .unwrap();
        assert_eq!(u1.accepted, Some(true));

        let u2 = two.poll(None).unwrap();
        assert_eq!(u2.revision, 1);
        assert_eq!(u2.changes.len(), 1);
        assert_eq!(u2.playback.as_ref().map(|p| p.tempo), Some(145.0));

        // Audio asset upload from peer1 -> host -> download to peer2 over P2P.
        let store1 = assets::Store::new(root.join("peer1").join("assets")).unwrap();
        let store2 = assets::Store::new(root.join("peer2").join("assets")).unwrap();
        let audio_bytes = vec![42u8; 200_000];
        let mut hasher = Sha256::new();
        hasher.update(&audio_bytes);
        let hex: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
        let asset_id = format!("{hex}.wav");
        let total = audio_bytes.len() as u64;
        for (i, chunk) in audio_bytes.chunks(CHUNK).enumerate() {
            store1
                .write(&asset_id, total, (i * CHUNK) as u64, chunk)
                .unwrap();
        }
        one.upload_audio(&asset_id, &store1, &AtomicBool::new(false))
            .unwrap();
        two.download_audio(&asset_id, &store2, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(std::fs::read(store2.root.join(&asset_id)).unwrap(), audio_bytes);

        // Reconnect deduplication over P2P.
        drop(one);
        let mut one = Client::connect(&invite, &peer1_id, "RemoteOne").unwrap();
        let p_dup = Proposal {
            seq: 1,
            changes: vec![Change {
                key: "pattern/2".into(),
                before: None,
                after: Some(json!([48])),
            }],
        };
        assert_eq!(one.poll(Some(p_dup.clone())).unwrap().accepted, Some(true));
        drop(one);
        let mut one = Client::connect(&invite, &peer1_id, "RemoteOne").unwrap();
        assert_eq!(one.poll(Some(p_dup)).unwrap().revision, 2);

        // Wrong secret rejected over P2P.
        let wrong = Invitation {
            secret: random_id().unwrap(),
            ..invite.clone()
        };
        assert!(Client::connect(&wrong, &random_id().unwrap(), "Intruder").is_err());

        drop(local_client);
        drop(one);
        drop(two);
        drop(host);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn p2p_iroh_concurrent_conflict_and_atomic_rejection() {
        let root = std::env::temp_dir().join(format!("ml-p2p-conflict-{}", random_id().unwrap()));
        std::fs::create_dir_all(&root).unwrap();
        let package = root.join("host.zip");
        std::fs::write(&package, b"conflict test").unwrap();

        let host = Host::start_p2p(package, Document::new(), valid).unwrap();
        let invite = Invitation::parse(&host.invitation.encode().unwrap()).unwrap();
        let mut a = Client::connect(&invite, &random_id().unwrap(), "PeerA").unwrap();
        let mut b = Client::connect(&invite, &random_id().unwrap(), "PeerB").unwrap();

        let edit_a = Proposal {
            seq: 1,
            changes: vec![Change {
                key: "mixer/1/pan".into(),
                before: None,
                after: Some(json!(0.5)),
            }],
        };
        let edit_b = Proposal {
            seq: 1,
            changes: vec![Change {
                key: "mixer/1/pan".into(),
                before: None,
                after: Some(json!(-0.5)),
            }],
        };
        assert_eq!(a.poll(Some(edit_a)).unwrap().accepted, Some(true));
        let res_b = b.poll(Some(edit_b)).unwrap();
        assert_eq!(res_b.accepted, Some(false));
        assert!(res_b.rejection.is_some());
        assert_eq!(res_b.revision, 1);
        assert_eq!(b.document.get("mixer/1/pan"), Some(&json!(0.5)));

        drop(a);
        drop(b);
        drop(host);
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn p2p_iroh_unreachable_host_fails_cleanly() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let random_id_key = rt.block_on(async {
            let ep = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                .bind()
                .await
                .unwrap();
            let id = ep.id();
            ep.close().await;
            id
        });
        let unreachable = Invitation {
            address: format!("p2p:{}", random_id_key.fmt_short()),
            certificate: "AQID".into(),
            secret: random_id().unwrap(),
            protocol: PROTOCOL,
            p2p: Some(iroh::EndpointAddr::new(random_id_key)),
        };
        let res = Client::connect(&unreachable, &random_id().unwrap(), "Guest");
        assert!(res.is_err());
    }
}
