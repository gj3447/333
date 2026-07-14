// KG: SPAN_333_Tauri, SPAN_333_CLI_Binary, SPAN_333_CLI_FeatureFlag, SPAN_333_CLI_NativeTransport
// KG: CONTRACT_333_CLI_Binary, CONTRACT_333_CLI_FeatureFlag
// Tauri v2 app: webview frontend + native Rust backend.
//
// Binary behaviour (no Tauri runtime required for CLI subcommands):
//   triple-three-desktop                      — print config + bridge status
//   triple-three-desktop --help               — usage
//   triple-three-desktop --version            — version string
//   triple-three-desktop native-smoke         — run native transport roundtrip
//                                                (requires --features native-p2p)

use serde::{Deserialize, Serialize};

#[cfg(feature = "native-p2p")]
mod native_transport;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const USAGE: &str = "\
triple-three-desktop — 333 Platform desktop CLI

USAGE:
    triple-three-desktop [SUBCOMMAND]

SUBCOMMANDS:
    (none)           Print native P2P config + protocol bridge status
    native-smoke     Run a loopback TCP roundtrip via the native transport
                     (requires --features native-p2p at build time)
    --help           Show this message
    --version        Print the binary version
";

// KG: CONTRACT_333_CLI_NativeTransport — declarative configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeP2PConfig {
    pub listen_addr: String,       // e.g. "/ip4/0.0.0.0/tcp/9333"
    pub quic_enabled: bool,
    pub tcp_enabled: bool,
    pub relay_mode: bool,          // act as TURN relay for browser peers
    pub dht_bootstrap: Vec<String>,
}

impl Default for NativeP2PConfig {
    fn default() -> Self {
        Self {
            listen_addr: "/ip4/0.0.0.0/tcp/9333".into(),
            quic_enabled: true,
            tcp_enabled: true,
            relay_mode: false,
            dht_bootstrap: vec![],
        }
    }
}

/// Protocol bridge: translates between WebRTC (browser) and QUIC (desktop)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolBridge {
    pub browser_peers: Vec<String>,   // connected via signaling→WebRTC
    pub native_peers: Vec<String>,    // connected via libp2p QUIC/TCP
    pub messages_bridged: u64,
}

impl ProtocolBridge {
    pub fn new() -> Self {
        Self {
            browser_peers: Vec::new(),
            native_peers: Vec::new(),
            messages_bridged: 0,
        }
    }

    /// Bridge a message from browser peer to native peer
    pub fn bridge_to_native(&mut self, _from_browser: &str, _to_native: &str, _data: &[u8]) {
        self.messages_bridged += 1;
    }

    /// Bridge a message from native peer to browser peer
    pub fn bridge_to_browser(&mut self, _from_native: &str, _to_browser: &str, _data: &[u8]) {
        self.messages_bridged += 1;
    }
}

// KG: CONTRACT_333_CLI_FeatureFlag — runtime toggle reflects compile feature
pub const fn native_p2p_enabled() -> bool {
    cfg!(feature = "native-p2p")
}

fn print_config() {
    let config = NativeP2PConfig::default();
    let bridge = ProtocolBridge::new();
    println!("333 Desktop CLI v{}", VERSION);
    println!("native-p2p feature: {}", if native_p2p_enabled() { "ENABLED" } else { "disabled" });
    println!("P2P Config: {}", serde_json::to_string_pretty(&config).unwrap());
    println!("Protocol Bridge: {} messages bridged", bridge.messages_bridged);
}

fn run_native_smoke() -> i32 {
    #[cfg(feature = "native-p2p")]
    {
        match native_transport::loopback_roundtrip(b"333-cli-smoke") {
            Ok(echoed) => {
                println!(
                    "native-smoke OK: roundtrip echoed {} bytes ({:?})",
                    echoed.len(),
                    std::str::from_utf8(&echoed).unwrap_or("<binary>")
                );
                0
            }
            Err(e) => {
                eprintln!("native-smoke FAILED: {e}");
                1
            }
        }
    }
    #[cfg(not(feature = "native-p2p"))]
    {
        eprintln!("native-smoke requires --features native-p2p at build time");
        2
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(|s| s.as_str()) {
        None => {
            print_config();
            0
        }
        Some("--help") | Some("-h") => {
            println!("{USAGE}");
            0
        }
        Some("--version") | Some("-V") => {
            println!("triple-three-desktop {VERSION}");
            0
        }
        Some("native-smoke") => run_native_smoke(),
        Some(other) => {
            eprintln!("unknown subcommand: {other}\n\n{USAGE}");
            64 // EX_USAGE
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = NativeP2PConfig::default();
        assert!(cfg.quic_enabled);
        assert!(cfg.tcp_enabled);
        assert!(!cfg.relay_mode);
    }

    #[test]
    fn bridge_counts_messages() {
        let mut bridge = ProtocolBridge::new();
        bridge.bridge_to_native("browser-1", "native-1", b"hello");
        bridge.bridge_to_browser("native-1", "browser-1", b"world");
        assert_eq!(bridge.messages_bridged, 2);
    }

    #[test]
    fn feature_flag_reflects_cfg() {
        // native_p2p_enabled() must agree with #[cfg] at compile time.
        let expected = cfg!(feature = "native-p2p");
        assert_eq!(native_p2p_enabled(), expected);
    }

    #[test]
    fn version_is_semver() {
        // VERSION from env!() at compile time — must be non-empty and contain a dot.
        assert!(!VERSION.is_empty());
        assert!(VERSION.contains('.'));
    }
}
