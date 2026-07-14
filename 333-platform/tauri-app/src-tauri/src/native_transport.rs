// KG: SPAN_333_CLI_NativeTransport, CONTRACT_333_CLI_NativeTransport, TASK_333_CLI_NativeTransport
//
// Native transport stub — only compiled under the `native-p2p` feature.
// Provides a minimal TCP listener + connect primitive to prove the
// feature-flagged code path compiles and executes in isolation.  Full
// libp2p QUIC wiring is deferred (tracked as a follow-up seed).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct NativeTransportConfig {
    pub listen_host: String,
    pub listen_port: u16,
}

impl Default for NativeTransportConfig {
    fn default() -> Self {
        Self {
            listen_host: "127.0.0.1".into(),
            listen_port: 0, // OS-assigned ephemeral
        }
    }
}

/// Bind a TCP listener and return the bound (host, port).  Callers
/// drop the returned listener to release the socket.
pub fn bind(cfg: &NativeTransportConfig) -> std::io::Result<(TcpListener, String, u16)> {
    let listener = TcpListener::bind((cfg.listen_host.as_str(), cfg.listen_port))?;
    let local = listener.local_addr()?;
    Ok((listener, local.ip().to_string(), local.port()))
}

/// Roundtrip a single message over a loopback TCP stream — proves the
/// transport can send + receive bytes end-to-end.
pub fn loopback_roundtrip(msg: &[u8]) -> std::io::Result<Vec<u8>> {
    let cfg = NativeTransportConfig::default();
    let (listener, host, port) = bind(&cfg)?;

    let msg_owned = msg.to_vec();
    let accept_handle = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        listener.set_nonblocking(false)?;
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut buf = vec![0u8; msg_owned.len()];
        stream.read_exact(&mut buf)?;
        stream.write_all(&buf)?;
        Ok(buf)
    });

    let mut client = TcpStream::connect((host.as_str(), port))?;
    client.set_read_timeout(Some(Duration::from_secs(2)))?;
    client.write_all(msg)?;
    let mut echo = vec![0u8; msg.len()];
    client.read_exact(&mut echo)?;
    accept_handle.join().expect("server panicked")?;
    Ok(echo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_assigns_ephemeral_port() {
        let (listener, host, port) = bind(&NativeTransportConfig::default()).unwrap();
        assert_eq!(host, "127.0.0.1");
        assert!(port > 0);
        drop(listener);
    }

    #[test]
    fn roundtrip_echoes_payload() {
        let payload = b"333-native-hello";
        let echoed = loopback_roundtrip(payload).unwrap();
        assert_eq!(echoed, payload);
    }
}
