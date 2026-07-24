//! p333 — 333 v2 substrate CLI.
//!
//! Thin wrapper over what the repo already ships, never a reinvention:
//!
//! - `p333 gate run` drives the official cross-language suite `verify/run_gates.sh`
//!   (every ooptdd gate GREEN over a real Rust-emitted trace, RED over an injected
//!   adversary, restored GREEN — the forbid/invariant gates proven to fire).
//! - `p333 gate inject <trace> <gate> <event-json>` reproduces ONE injected-adversary
//!   RED check through the same ooptdd route the suite uses.
//! - `p333 receipt verify <receipt.json>` re-judges a committed receipt artifact
//!   against its declared schema fields and every sha256 binding (a tampered or
//!   drifted file turns the receipt RED — receipts, not claims).
//!
//! Exit codes: 0 = PASS (gate green / injection fired / receipt valid),
//! 1 = FAIL (gate red / injection did NOT fire / receipt invalid),
//! 2 = usage error or infrastructure failure (never masquerades as a verdict).

use std::path::PathBuf;
use std::process::ExitCode;

mod gate;
mod receipt;

/// Repo root = `crates/p333-cli/../..`, resolved at compile time so the binary
/// works from any cwd (the same "run from anywhere" contract `run_gates.sh` keeps).
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    GateRun,
    GateInject { trace: PathBuf, gate: PathBuf, event: String },
    ReceiptVerify { path: PathBuf, root: Option<PathBuf> },
    Help,
}

const USAGE: &str = "\
p333 — 333 v2 substrate CLI

usage:
  p333 gate run
      Run the official cross-language gate suite (sh verify/run_gates.sh):
      every ooptdd gate GREEN over a real trace, RED over an injected
      adversary, restored GREEN. Needs a container runtime + ooptdd checkout.
  p333 gate inject <trace.jsonl> <gate.yaml> <event-json>
      Inject one adversarial event into a copy of the trace and assert the
      ooptdd gate fires RED (exit 1 from the verifier = PASS here).
  p333 receipt verify <receipt.json> [--root <dir>]
      Re-validate a receipt artifact: declared schema fields present, verdicts
      coherent, and every sha256 binding re-hashed against the file on disk
      (repo-relative to --root, default: this repo).
  p333 help

exit: 0 pass, 1 fail, 2 usage/infra error";

fn parse_args(args: &[String]) -> Result<Command, String> {
    let rest = args.get(1..).unwrap_or(&[]);
    match rest {
        [] => Err(USAGE.into()),
        [cmd] if cmd == "help" || cmd == "--help" || cmd == "-h" => Ok(Command::Help),
        [cmd, sub] if cmd == "gate" && sub == "run" => Ok(Command::GateRun),
        [cmd, sub, trace, gate, event] if cmd == "gate" && sub == "inject" => {
            Ok(Command::GateInject {
                trace: PathBuf::from(trace),
                gate: PathBuf::from(gate),
                event: event.clone(),
            })
        }
        [cmd, sub, path] if cmd == "receipt" && sub == "verify" => {
            Ok(Command::ReceiptVerify { path: PathBuf::from(path), root: None })
        }
        [cmd, sub, path, flag, root] if cmd == "receipt" && sub == "verify" && flag == "--root" => {
            Ok(Command::ReceiptVerify {
                path: PathBuf::from(path),
                root: Some(PathBuf::from(root)),
            })
        }
        _ => Err(format!("unknown arguments: {}\n\n{USAGE}", rest.join(" "))),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let cmd = match parse_args(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    match cmd {
        Command::Help => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Command::GateRun => match gate::run(&repo_root()) {
            Ok(0) => ExitCode::SUCCESS,
            Ok(_) => ExitCode::FAILURE,
            Err(e) => {
                eprintln!("ERROR: {e}");
                ExitCode::from(2)
            }
        },
        Command::GateInject { trace, gate, event } => {
            match gate::inject(&repo_root(), &trace, &gate, &event) {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => ExitCode::FAILURE,
                Err(e) => {
                    eprintln!("ERROR: {e}");
                    ExitCode::from(2)
                }
            }
        }
        Command::ReceiptVerify { path, root } => {
            let root = root.unwrap_or_else(repo_root);
            match receipt::verify_receipt(&path, &root) {
                Ok(report) => {
                    report.print();
                    if report.ok() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    }
                }
                Err(e) => {
                    eprintln!("ERROR: {e}");
                    ExitCode::from(2)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(rest: &[&str]) -> Vec<String> {
        std::iter::once("p333".to_string()).chain(rest.iter().map(|s| s.to_string())).collect()
    }

    #[test]
    fn no_args_is_usage_error() {
        assert!(parse_args(&argv(&[])).is_err());
    }

    #[test]
    fn parses_gate_run() {
        assert_eq!(parse_args(&argv(&["gate", "run"])).unwrap(), Command::GateRun);
    }

    #[test]
    fn parses_gate_inject() {
        let cmd = parse_args(&argv(&["gate", "inject", "t.jsonl", "g.yaml", "{\"cid\":\"c\",\"event\":\"e\"}"])).unwrap();
        assert_eq!(
            cmd,
            Command::GateInject {
                trace: PathBuf::from("t.jsonl"),
                gate: PathBuf::from("g.yaml"),
                event: "{\"cid\":\"c\",\"event\":\"e\"}".to_string(),
            }
        );
    }

    #[test]
    fn gate_inject_missing_event_is_error() {
        assert!(parse_args(&argv(&["gate", "inject", "t.jsonl", "g.yaml"])).is_err());
    }

    #[test]
    fn parses_receipt_verify_default_root() {
        let cmd = parse_args(&argv(&["receipt", "verify", "r.json"])).unwrap();
        assert_eq!(cmd, Command::ReceiptVerify { path: PathBuf::from("r.json"), root: None });
    }

    #[test]
    fn parses_receipt_verify_with_root() {
        let cmd = parse_args(&argv(&["receipt", "verify", "r.json", "--root", "/tmp/x"])).unwrap();
        assert_eq!(
            cmd,
            Command::ReceiptVerify { path: PathBuf::from("r.json"), root: Some(PathBuf::from("/tmp/x")) }
        );
    }

    #[test]
    fn receipt_verify_missing_path_is_error() {
        assert!(parse_args(&argv(&["receipt", "verify"])).is_err());
    }

    #[test]
    fn unknown_subcommand_is_error() {
        assert!(parse_args(&argv(&["bogus"])).is_err());
        assert!(parse_args(&argv(&["gate", "bogus"])).is_err());
    }
}
