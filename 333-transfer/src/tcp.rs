// KG: transport-plan Step 8 (2026-07-14)
//
// Real TCP I/O behind `AuthorityNet`, mirroring `333-wire`'s pattern:
// `std::net::{TcpListener, TcpStream}`, hand-rolled length-prefixed frames
// (u32 big-endian length + body), listener thread + per-connection reader
// threads, no tokio / no serde.
//
// Frame body = `wire::encode_authority_msg` (1-byte tag + domain-tagged body).

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::authority::{Certificate, Vote};
use crate::net::{AuthorityMsg, AuthorityNet, NetError};
use crate::wire::{decode_authority_msg, encode_authority_msg};
use crate::Transfer;

const READ_TIMEOUT: Duration = Duration::from_millis(200);
const CONNECT_ATTEMPTS: u32 = 40;
const CONNECT_BACKOFF: Duration = Duration::from_millis(10);

/// Shared state for one TCP authority peer (listener + outbound peers + inbox).
struct TcpInner {
    id: String,
    addr: SocketAddr,
    peers: Mutex<Vec<TcpStream>>,
    inbox: Mutex<VecDeque<AuthorityMsg>>,
    stop: AtomicBool,
    listener: Mutex<Option<JoinHandle<()>>>,
    readers: Mutex<Vec<JoinHandle<()>>>,
}

/// One TCP peer: binds `127.0.0.1:0`, accepts inbound framed messages, dials
/// outbound peers for broadcasts. Pair with [`TcpEndpoint`] for [`AuthorityNet`].
pub struct TcpAuthorityNet {
    inner: Arc<TcpInner>,
}

/// Per-peer handle implementing [`AuthorityNet`] (cheap clone via `Arc`).
#[derive(Clone)]
pub struct TcpEndpoint {
    inner: Arc<TcpInner>,
}

impl TcpAuthorityNet {
    /// Bind an ephemeral port on loopback and start the accept loop.
    pub fn bind(id: impl Into<String>) -> io::Result<Self> {
        let id = id.into();
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        // Short accept timeout so shutdown can join without hanging forever.
        let _ = listener.set_nonblocking(false);
        let inner = Arc::new(TcpInner {
            id,
            addr,
            peers: Mutex::new(Vec::new()),
            inbox: Mutex::new(VecDeque::new()),
            stop: AtomicBool::new(false),
            listener: Mutex::new(None),
            readers: Mutex::new(Vec::new()),
        });
        let handle = spawn_listener(Arc::clone(&inner), listener);
        *inner.listener.lock().expect("listener lock") = Some(handle);
        Ok(Self { inner })
    }

    pub fn id(&self) -> &str {
        &self.inner.id
    }

    pub fn addr(&self) -> SocketAddr {
        self.inner.addr
    }

    /// Cheap cloneable endpoint for the Steps 5–7 generic drivers.
    pub fn endpoint(&self) -> TcpEndpoint {
        TcpEndpoint {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Dial one peer (bounded retries + small backoff for listener startup).
    pub fn connect_peer(&self, addr: SocketAddr) -> Result<(), NetError> {
        if addr == self.inner.addr {
            return Ok(());
        }
        let stream = connect_with_retry(addr)?;
        configure_stream(&stream)?;
        self.inner
            .peers
            .lock()
            .expect("peers lock")
            .push(stream);
        Ok(())
    }

    /// Full-mesh dial: connect to every address except self.
    pub fn connect_all(&self, addrs: &[SocketAddr]) -> Result<(), NetError> {
        for &a in addrs {
            self.connect_peer(a)?;
        }
        Ok(())
    }

    /// Stop flag + join threads; ignore already-closed sockets.
    pub fn shutdown(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        // Unblock accept by connecting to self (best-effort).
        let _ = TcpStream::connect(self.inner.addr);
        // Drop outbound peers so peer readers exit.
        self.inner.peers.lock().expect("peers lock").clear();
        if let Some(h) = self.inner.listener.lock().expect("listener lock").take() {
            let _ = h.join();
        }
        let readers = std::mem::take(&mut *self.inner.readers.lock().expect("readers lock"));
        for h in readers {
            let _ = h.join();
        }
    }
}

impl Drop for TcpAuthorityNet {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl TcpEndpoint {
    pub fn id(&self) -> &str {
        &self.inner.id
    }

    pub fn addr(&self) -> SocketAddr {
        self.inner.addr
    }
}

impl AuthorityNet for TcpEndpoint {
    fn broadcast_order(&self, t: Transfer) -> Result<(), NetError> {
        self.write_all(&AuthorityMsg::Order(t))
    }

    fn broadcast_vote(&self, v: Vote) -> Result<(), NetError> {
        self.write_all(&AuthorityMsg::Vote(v))
    }

    fn broadcast_cert(&self, c: Certificate) -> Result<(), NetError> {
        self.write_all(&AuthorityMsg::Cert(c))
    }

    fn poll(&self) -> Vec<AuthorityMsg> {
        let mut g = self.inner.inbox.lock().expect("inbox lock");
        g.drain(..).collect()
    }
}

impl TcpEndpoint {
    fn write_all(&self, msg: &AuthorityMsg) -> Result<(), NetError> {
        if self.inner.stop.load(Ordering::SeqCst) {
            return Err(NetError::Closed);
        }
        let frame = encode_tcp_frame(msg);
        let mut peers = self.inner.peers.lock().expect("peers lock");
        if peers.is_empty() {
            return Ok(());
        }
        let mut ok = 0usize;
        let mut last_err: Option<String> = None;
        for p in peers.iter_mut() {
            match p.write_all(&frame).and_then(|_| p.flush()) {
                Ok(()) => ok += 1,
                Err(e) => last_err = Some(e.to_string()),
            }
        }
        if ok == 0 {
            return Err(NetError::Io(
                last_err.unwrap_or_else(|| "all peer writes failed".into()),
            ));
        }
        // Best-effort full-mesh fan-out: succeed if at least one peer got the frame.
        Ok(())
    }
}

fn encode_tcp_frame(msg: &AuthorityMsg) -> Vec<u8> {
    let body = encode_authority_msg(msg);
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

fn read_tcp_frame<R: Read>(stream: &mut R) -> io::Result<AuthorityMsg> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let body_len = u32::from_be_bytes(len_buf) as usize;
    // Guard absurd lengths (DoS / corruption).
    if body_len > 16 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut body = vec![0u8; body_len];
    stream.read_exact(&mut body)?;
    decode_authority_msg(&body).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidData, format!("authority frame: {e:?}"))
    })
}

fn configure_stream(stream: &TcpStream) -> Result<(), NetError> {
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|e| NetError::Io(e.to_string()))?;
    stream
        .set_write_timeout(Some(READ_TIMEOUT))
        .map_err(|e| NetError::Io(e.to_string()))?;
    stream
        .set_nodelay(true)
        .map_err(|e| NetError::Io(e.to_string()))?;
    Ok(())
}

fn connect_with_retry(addr: SocketAddr) -> Result<TcpStream, NetError> {
    let mut last = None;
    for attempt in 0..CONNECT_ATTEMPTS {
        match TcpStream::connect(addr) {
            Ok(s) => return Ok(s),
            Err(e) => {
                last = Some(e);
                if attempt + 1 < CONNECT_ATTEMPTS {
                    thread::sleep(CONNECT_BACKOFF);
                }
            }
        }
    }
    Err(NetError::Io(format!(
        "connect {addr} failed after {CONNECT_ATTEMPTS} attempts: {:?}",
        last.map(|e| e.to_string())
    )))
}

fn spawn_listener(inner: Arc<TcpInner>, listener: TcpListener) -> JoinHandle<()> {
    // Accept timeout: use set_nonblocking + sleep, or SO_RCVTIMEO via
    // read timeout on a dual-purpose approach. On Unix we can set_read_timeout
    // is not on TcpListener in std — unblock via self-connect on shutdown.
    thread::spawn(move || {
        for conn in listener.incoming() {
            if inner.stop.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = conn else { continue };
            if inner.stop.load(Ordering::SeqCst) {
                break;
            }
            let _ = configure_stream(&stream);
            let inbox = Arc::clone(&inner);
            let handle = thread::spawn(move || {
                let mut stream = stream;
                loop {
                    if inbox.stop.load(Ordering::SeqCst) {
                        break;
                    }
                    match read_tcp_frame(&mut stream) {
                        Ok(msg) => {
                            inbox
                                .inbox
                                .lock()
                                .expect("inbox lock")
                                .push_back(msg);
                        }
                        Err(e) => {
                            // Timeout: continue reading (allows stop check).
                            if e.kind() == io::ErrorKind::WouldBlock
                                || e.kind() == io::ErrorKind::TimedOut
                            {
                                continue;
                            }
                            break;
                        }
                    }
                }
            });
            inner
                .readers
                .lock()
                .expect("readers lock")
                .push(handle);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::{Authority, Certificate, Committee};
    use crate::net::AuthorityNet;
    use ed25519_dalek::SigningKey;

    fn tx(from: &str, seq: u64, to: &str, amount: u128) -> Transfer {
        Transfer {
            from: from.into(),
            from_seq: seq,
            to: to.into(),
            amount,
        }
    }

    fn key(i: u8) -> SigningKey {
        SigningKey::from_bytes(&[i; 32])
    }

    #[test]
    fn tcp_order_round_trip_two_peers() {
        let a = TcpAuthorityNet::bind("a").unwrap();
        let b = TcpAuthorityNet::bind("b").unwrap();
        let addrs = [a.addr(), b.addr()];
        a.connect_all(&addrs).unwrap();
        b.connect_all(&addrs).unwrap();

        let ea = a.endpoint();
        let eb = b.endpoint();
        ea.broadcast_order(tx("alice", 0, "bob", 1)).unwrap();

        // Poll with small retries for reader thread.
        let mut got = Vec::new();
        for _ in 0..50 {
            got.extend(eb.poll());
            if !got.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            matches!(got.as_slice(), [AuthorityMsg::Order(_)]),
            "got {got:?}"
        );
        a.shutdown();
        b.shutdown();
    }

    #[test]
    fn tcp_frame_preserves_valid_certificate_bytes() {
        // Encode path used on the wire must still yield is_valid after decode.
        let t = tx("alice", 0, "bob", 10);
        let auth: Vec<Authority> = (0..4u8)
            .map(|i| Authority::new(format!("a{i}"), key(i)))
            .collect();
        let committee =
            Committee::new(auth.iter().map(|a| (a.id().clone(), a.verifying_key()))).unwrap();
        let mut auth = auth;
        let votes: Vec<_> = auth
            .iter_mut()
            .take(3)
            .map(|a| a.handle(&t).unwrap())
            .collect();
        let cert = Certificate::assemble(t, votes, &committee).unwrap();
        let frame = encode_tcp_frame(&AuthorityMsg::Cert(cert.clone()));
        // Strip length prefix and decode body as wire does on the reader.
        let body = &frame[4..];
        let back = decode_authority_msg(body).unwrap();
        match back {
            AuthorityMsg::Cert(c) => assert!(c.is_valid(&committee)),
            _ => panic!("cert"),
        }
    }
}
