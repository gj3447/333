//! `p333 gate …` — wraps the repo's own gate artifacts; the gates stay the
//! single source of truth. `run` shells out to `verify/run_gates.sh` unchanged;
//! `inject` reproduces one RED check from that script (append an adversarial
//! envelope to a copy of the trace, judge with the same ooptdd route, want
//! exit 1) so a single adversary can be probed without the full suite.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `p333 gate run` — the official cross-language suite (GREEN over real traces,
/// RED over injected adversaries, restored GREEN). Streams stdio through and
/// returns the script's exit code.
pub fn run(root: &Path) -> Result<i32, String> {
    let script = root.join("verify/run_gates.sh");
    if !script.is_file() {
        return Err(format!("gate suite not found at {}", script.display()));
    }
    let status = Command::new("sh")
        .arg(&script)
        .current_dir(root)
        .status()
        .map_err(|e| format!("failed to spawn sh {}: {e}", script.display()))?;
    Ok(status.code().unwrap_or(2))
}

/// Parse the adversarial event as an ooptdd envelope — reuses
/// `p333_ltdd::Event::from_ooptdd_json` so the CLI speaks exactly the trace
/// vocabulary the gates judge (cid/cycle_id + event, structured attrs).
pub fn parse_adversary(s: &str) -> Result<serde_json::Value, String> {
    let v: serde_json::Value =
        serde_json::from_str(s).map_err(|e| format!("adversary is not valid JSON: {e}"))?;
    p333_ltdd::Event::from_ooptdd_json(&v)
        .ok_or_else(|| "adversary is not an ooptdd envelope (needs cid/cycle_id + event)".to_string())?;
    Ok(v)
}

/// Build the injected trace: every line of `trace` plus the adversary envelope,
/// one envelope per line (the ooptdd JSONL wire shape). Returns the temp path.
pub fn build_injected_trace(trace: &Path, adversary: &serde_json::Value) -> Result<PathBuf, String> {
    let base = std::fs::read_to_string(trace)
        .map_err(|e| format!("cannot read trace {}: {e}", trace.display()))?;
    let mut out = base;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&adversary.to_string());
    out.push('\n');
    let path = std::env::temp_dir().join(format!("p333-inject-{}-{}.jsonl", std::process::id(), adversary["event"]));
    std::fs::write(&path, out).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// Judge a trace with the repo's own cross-language route — the same invocation
/// `verify/run_gates.sh` uses: `uv run --frozen --extra dev python
/// verify/ooptdd_verify.py <trace> <gate>` inside the ooptdd checkout
/// (`OOPTDD_PATH`, else `<root>/../ooptdd`). Returns the verifier's exit code
/// (0 GREEN / 1 RED / 2 inconclusive).
pub fn judge(root: &Path, trace: &Path, gate: &Path) -> Result<i32, String> {
    let ooptdd = std::env::var_os("OOPTDD_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../ooptdd"));
    if !ooptdd.join("pyproject.toml").is_file() {
        return Err(format!(
            "ooptdd checkout not found at '{}'; set OOPTDD_PATH to its source checkout",
            ooptdd.display()
        ));
    }
    let cache = std::env::var_os("OOPTDD_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| ooptdd.join(".uv-cache"));
    let verifier = root.join("verify/ooptdd_verify.py");
    let status = Command::new("uv")
        .args(["run", "--frozen", "--extra", "dev", "python"])
        .arg(&verifier)
        .arg(trace)
        .arg(gate)
        .current_dir(&ooptdd)
        .env("UV_CACHE_DIR", &cache)
        .status()
        .map_err(|e| format!("failed to spawn uv (is uv on PATH?): {e}"))?;
    Ok(status.code().unwrap_or(2))
}

/// `p333 gate inject` — Ok(true) = PASS (the gate fired RED over the injected
/// adversary), Ok(false) = FAIL (the gate stayed GREEN: the forbid/invariant
/// did not fire), Err = usage/infra failure (never a verdict).
pub fn inject(root: &Path, trace: &Path, gate: &Path, event: &str) -> Result<bool, String> {
    let adversary = parse_adversary(event)?;
    let red_trace = build_injected_trace(trace, &adversary)?;
    let rc = judge(root, &red_trace, gate);
    let _ = std::fs::remove_file(&red_trace);
    match rc? {
        1 => {
            println!("PASS  injected adversary turned the gate RED (exit 1) — the gate fires");
            Ok(true)
        }
        0 => {
            println!("FAIL  gate stayed GREEN over an injected adversary — the gate did NOT fire");
            Ok(false)
        }
        other => Err(format!("ooptdd verdict inconclusive/infra (exit {other}) — not a verdict")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adversary_accepts_cid_envelope() {
        let v = parse_adversary("{\"cid\":\"c1\",\"event\":\"replica_diverged\",\"replica\":\"z\"}").unwrap();
        assert_eq!(v["event"], "replica_diverged");
        assert_eq!(v["replica"], "z");
    }

    #[test]
    fn adversary_accepts_cycle_id_fallback() {
        assert!(parse_adversary("{\"cycle_id\":\"c1\",\"event\":\"replay_diverged\"}").is_ok());
    }

    #[test]
    fn adversary_rejects_missing_event() {
        assert!(parse_adversary("{\"cid\":\"c1\"}").is_err());
    }

    #[test]
    fn adversary_rejects_missing_cid() {
        assert!(parse_adversary("{\"event\":\"replica_diverged\"}").is_err());
    }

    #[test]
    fn adversary_rejects_non_object_and_non_json() {
        assert!(parse_adversary("[1,2]").is_err());
        assert!(parse_adversary("not json").is_err());
    }

    #[test]
    fn injected_trace_appends_one_envelope_line() {
        let dir = tempfile::tempdir().unwrap();
        let trace = dir.path().join("base.jsonl");
        std::fs::write(&trace, "{\"cid\":\"c\",\"event\":\"a\"}\n{\"cid\":\"c\",\"event\":\"b\"}").unwrap();
        let adv = parse_adversary("{\"cid\":\"c\",\"event\":\"replica_diverged\"}").unwrap();
        let red = build_injected_trace(&trace, &adv).unwrap();
        let lines: Vec<String> = std::fs::read_to_string(&red)
            .unwrap()
            .lines()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(lines.len(), 3);
        // The appended line is a well-formed envelope the gates can judge.
        let v: serde_json::Value = serde_json::from_str(&lines[2]).unwrap();
        assert!(p333_ltdd::Event::from_ooptdd_json(&v).is_some());
        let _ = std::fs::remove_file(&red);
    }

    #[test]
    fn injected_trace_handles_missing_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let trace = dir.path().join("base.jsonl");
        std::fs::write(&trace, "{\"cid\":\"c\",\"event\":\"a\"}").unwrap();
        let adv = parse_adversary("{\"cid\":\"c\",\"event\":\"x\"}").unwrap();
        let red = build_injected_trace(&trace, &adv).unwrap();
        assert_eq!(std::fs::read_to_string(&red).unwrap().lines().count(), 2);
        let _ = std::fs::remove_file(&red);
    }
}
