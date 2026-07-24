//! `p333 receipt verify` — re-judges a committed receipt artifact against its
//! declared schema and every sha256 binding, re-hashing the files on disk.
//! The receipt vocabulary is the repo's own (`verify/receipts/*.json`):
//!
//! - `symposium-ooptdd-receipt/v1`: spec / producer / positive(green) /
//!   negative_oracle(red, restored) / source_binding — each `path` field pinned
//!   by a sibling `sha256`.
//! - `pi-cycle/v1`: coordination receipt; `measurement.receipt_path` pinned by
//!   `measurement.receipt_sha256`.
//!
//! A tampered hash, a drifted file, a missing referenced path, or an incoherent
//! verdict turns the receipt RED (exit 1) — receipts, not claims.
//!
//! Pinned-revision semantics: a binding that no longer matches the CURRENT disk
//! is not automatically a provenance hole. When the receipt pins a producer
//! revision (`producer.git_head`, or `repo.git_head` for pi-cycle) and `--root`
//! sits inside a git repo, the blob at `<git_head>:<binding_path>` (path used
//! exactly as written — repo-root-relative, never re-anchored) is re-hashed. A
//! match there means *sealed-consistent*: the tree legitimately evolved after
//! the receipt was sealed. A blob that matches NEITHER disk nor the pinned rev
//! is the real provenance hole. Ancestry of the pinned rev is report-only —
//! blob hash equality is the verdict.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

/// One named check in the verification report.
#[derive(Debug)]
pub struct Check {
    pub label: String,
    pub ok: bool,
    pub detail: String,
}

/// The full report for one receipt artifact.
#[derive(Debug)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }

    pub fn print(&self) {
        for c in &self.checks {
            let mark = if c.ok { "PASS" } else { "FAIL" };
            if c.ok {
                println!("  {mark}  {}", c.label);
            } else {
                println!("  {mark}  {} — {}", c.label, c.detail);
            }
        }
        let failed = self.checks.iter().filter(|c| !c.ok).count();
        if failed == 0 {
            println!("== RECEIPT VALID: {} checks pass ==", self.checks.len());
        } else {
            println!("== RECEIPT INVALID: {failed}/{} checks fail ==", self.checks.len());
        }
    }
}

impl Check {
    fn pass(label: impl Into<String>) -> Self {
        Self { label: label.into(), ok: true, detail: String::new() }
    }
    fn fail(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self { label: label.into(), ok: false, detail: detail.into() }
    }
}

/// sha256 of a file, lowercase hex (the spelling the receipts record).
pub fn sha256_hex(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let digest = Sha256::digest(&bytes);
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// Nested lookup by dot path: `field(v, "positive.receipt_sha256")`.
fn field<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |cur, key| cur.get(key))
}

fn check_present(checks: &mut Vec<Check>, v: &Value, path: &str) {
    if field(v, path).is_some() {
        checks.push(Check::pass(format!("field {path} present")));
    } else {
        checks.push(Check::fail(format!("field {path} present"), "missing"));
    }
}

fn check_eq(checks: &mut Vec<Check>, v: &Value, path: &str, want: &Value) {
    match field(v, path) {
        Some(got) if got == want => checks.push(Check::pass(format!("{path} == {want}"))),
        Some(got) => checks.push(Check::fail(format!("{path} == {want}"), format!("got {got}"))),
        None => checks.push(Check::fail(format!("{path} == {want}"), "missing")),
    }
}

/// Outcome of the pinned-revision fallback for one binding.
enum PinnedLookup {
    /// The blob at the pinned rev hashes to the recorded sha256 (tree evolved after sealing).
    Sealed { ancestor_of_head: Option<bool> },
    /// The blob at the pinned rev hashes differently from the recorded sha256.
    Mismatch(String),
    /// The pinned rev is not in this repo's object store.
    RevAbsent,
    /// No such path at the pinned rev.
    BlobAbsent,
    /// git binary missing or `root` not inside a git repo.
    Unavailable,
}

/// The git repo toplevel containing `root`, if any.
fn git_toplevel(root: &Path) -> Option<std::path::PathBuf> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let t = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(t))
    }
}

/// Re-hash the blob `<git_head>:<binding_path>` in the git repo containing
/// `root`. The binding path is used exactly as the receipt writes it — the
/// `:path` form is repo-root-relative, so it is NEVER re-anchored (e.g. no
/// `substrate/` prefix) even when `root` is a subdirectory of the repo.
fn pinned_lookup(root: &Path, git_head: &str, binding_path: &str, recorded: &str) -> PinnedLookup {
    let Some(toplevel) = git_toplevel(root) else {
        return PinnedLookup::Unavailable;
    };
    let rev_exists = std::process::Command::new("git")
        .arg("-C")
        .arg(&toplevel)
        .args(["cat-file", "-e", git_head])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !rev_exists {
        return PinnedLookup::RevAbsent;
    }
    let out = match std::process::Command::new("git")
        .arg("-C")
        .arg(&toplevel)
        .arg("show")
        .arg(format!("{git_head}:{binding_path}"))
        .output()
    {
        Ok(o) => o,
        Err(_) => return PinnedLookup::Unavailable,
    };
    if !out.status.success() {
        return PinnedLookup::BlobAbsent;
    }
    let digest = Sha256::digest(&out.stdout);
    let actual = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
    if !actual.eq_ignore_ascii_case(recorded) {
        return PinnedLookup::Mismatch(actual);
    }
    // Report-only: ancestry is context, never a verdict input.
    let ancestor_of_head = std::process::Command::new("git")
        .arg("-C")
        .arg(&toplevel)
        .args(["merge-base", "--is-ancestor", git_head, "HEAD"])
        .status()
        .ok()
        .map(|s| s.success());
    PinnedLookup::Sealed { ancestor_of_head }
}

/// Re-hash `root/<file_field>` and compare with the recorded `<sha_field>`.
/// On disk mismatch, fall back to the pinned revision (`git_head`, when the
/// receipt carries one) before declaring a provenance hole.
fn check_hash_binding(
    checks: &mut Vec<Check>,
    v: &Value,
    root: &Path,
    file_field: &str,
    sha_field: &str,
    git_head: Option<&str>,
) {
    let label = format!("sha256 binding {file_field}");
    let (Some(file), Some(recorded)) = (
        field(v, file_field).and_then(Value::as_str),
        field(v, sha_field).and_then(Value::as_str),
    ) else {
        checks.push(Check::fail(label, format!("{file_field} or {sha_field} missing/not a string")));
        return;
    };
    let label = format!("{label} ({file})");
    let path = root.join(file);
    match sha256_hex(&path) {
        Ok(actual) if actual.eq_ignore_ascii_case(recorded) => {
            checks.push(Check::pass(label));
        }
        Ok(actual) => {
            let drift = || format!("recorded {recorded}, on-disk {actual}");
            match git_head {
                Some(rev) => match pinned_lookup(root, rev, file, recorded) {
                    PinnedLookup::Sealed { ancestor_of_head } => {
                        let note = match ancestor_of_head {
                            Some(true) => "; pinned rev is an ancestor of HEAD",
                            Some(false) => "; pinned rev is NOT an ancestor of HEAD",
                            None => "",
                        };
                        checks.push(Check::pass(format!(
                            "{label} — sealed-consistent (pinned git_head {} blob; disk evolved since{note})",
                            &rev[..rev.len().min(8)]
                        )));
                    }
                    PinnedLookup::Mismatch(blob) => checks.push(Check::fail(
                        label,
                        format!("recorded {recorded} matches neither disk ({actual}) nor pinned git_head ({blob}) — provenance hole"),
                    )),
                    PinnedLookup::RevAbsent => checks.push(Check::fail(
                        label,
                        format!("{}; pinned git_head {rev} not in history", drift()),
                    )),
                    PinnedLookup::BlobAbsent => checks.push(Check::fail(
                        label,
                        format!("{}; no blob {file} at pinned git_head {rev}", drift()),
                    )),
                    PinnedLookup::Unavailable => checks.push(Check::fail(
                        label,
                        format!("{} — tampered or drifted (git lookup unavailable; pinned git_head {rev} not checked)", drift()),
                    )),
                },
                None => checks.push(Check::fail(
                    label,
                    format!("{} — tampered or drifted", drift()),
                )),
            }
        }
        Err(e) => checks.push(Check::fail(label, e)),
    }
}

/// Verify one receipt artifact. `root` resolves the repo-relative paths the
/// receipt records. Err = the artifact is unreadable/not a known receipt
/// schema (usage-level, never a verdict).
pub fn verify_receipt(path: &Path, root: &Path) -> Result<Report, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read receipt {}: {e}", path.display()))?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| format!("receipt {} is not valid JSON: {e}", path.display()))?;
    match field(&v, "schema_version").and_then(Value::as_str) {
        Some("symposium-ooptdd-receipt/v1") => Ok(verify_ooptdd(&v, root)),
        Some("pi-cycle/v1") => Ok(verify_pi_cycle(&v, root)),
        Some(other) => Err(format!("unknown schema_version {other:?}")),
        None => Err("missing schema_version — not a known receipt schema".into()),
    }
}

fn verify_ooptdd(v: &Value, root: &Path) -> Report {
    let mut checks = Vec::new();
    check_eq(&mut checks, v, "template_only", &Value::Bool(false));
    for f in ["receipt_id", "cycle_id", "requirement_group", "correlation.cid", "producer.command"] {
        check_present(&mut checks, v, f);
    }
    check_eq(&mut checks, v, "producer.exit_code", &Value::from(0));
    match field(v, "requirements").and_then(Value::as_array) {
        Some(reqs) if !reqs.is_empty() => {
            checks.push(Check::pass(format!("requirements declared ({})", reqs.len())))
        }
        Some(_) => checks.push(Check::fail("requirements declared", "empty array")),
        None => checks.push(Check::fail("requirements declared", "missing/not an array")),
    }
    // Verdict coherence: positive green, injected negative red, fault restored.
    check_eq(&mut checks, v, "positive.observed_verdict", &Value::from("green"));
    check_eq(&mut checks, v, "negative_oracle.observed_verdict", &Value::from("red"));
    check_eq(&mut checks, v, "negative_oracle.restored", &Value::Bool(true));
    // Every declared sha256 binding re-hashed — against the disk first, then
    // against the blob at the pinned producer revision when the tree evolved.
    let git_head = field(v, "producer.git_head").and_then(Value::as_str);
    check_hash_binding(&mut checks, v, root, "spec.path", "spec.sha256", git_head);
    check_hash_binding(&mut checks, v, root, "positive.receipt_path", "positive.receipt_sha256", git_head);
    check_hash_binding(&mut checks, v, root, "negative_oracle.receipt_path", "negative_oracle.receipt_sha256", git_head);
    check_hash_binding(&mut checks, v, root, "source_binding.path", "source_binding.sha256", git_head);
    Report { checks }
}

fn verify_pi_cycle(v: &Value, root: &Path) -> Report {
    let mut checks = Vec::new();
    check_eq(&mut checks, v, "template_only", &Value::Bool(false));
    for f in ["cycle_id", "repo.git_head", "coordination.claim_status"] {
        check_present(&mut checks, v, f);
    }
    let git_head = field(v, "repo.git_head").and_then(Value::as_str);
    check_hash_binding(&mut checks, v, root, "measurement.receipt_path", "measurement.receipt_sha256", git_head);
    Report { checks }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A hermetic mini-repo: two referenced files + a receipt pinning them.
    struct Fixture {
        _dir: tempfile::TempDir,
        root: std::path::PathBuf,
        receipt_path: std::path::PathBuf,
        receipt: Value,
    }

    fn write_file(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn make_ooptdd_fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        write_file(&root, "crates/x/tests/spec.rs", b"the locked spec");
        write_file(&root, "verify/receipts/pos.log", b"4 passed; 0 failed");
        write_file(&root, "verify/receipts/neg.log", b"1 failed (injected)");
        write_file(&root, "crates/x/src/lib.rs", b"pub fn sync() {}");
        let sha = |rel: &str| sha256_hex(&root.join(rel)).unwrap();
        let receipt = json!({
            "schema_version": "symposium-ooptdd-receipt/v1",
            "template_only": false,
            "receipt_id": "x-receipt",
            "cycle_id": "test-cycle",
            "requirement_group": "X",
            "spec": {"path": "crates/x/tests/spec.rs", "sha256": sha("crates/x/tests/spec.rs")},
            "producer": {"command": "cargo test -p x", "exit_code": 0},
            "correlation": {"cid": "x-cid"},
            "requirements": [{"id": "R1", "role": "guard_mechanism", "event": "e"}],
            "positive": {
                "observed_verdict": "green",
                "receipt_path": "verify/receipts/pos.log",
                "receipt_sha256": sha("verify/receipts/pos.log"),
            },
            "negative_oracle": {
                "observed_verdict": "red",
                "receipt_path": "verify/receipts/neg.log",
                "receipt_sha256": sha("verify/receipts/neg.log"),
                "restored": true,
            },
            "source_binding": {"path": "crates/x/src/lib.rs", "symbol": "X::sync", "sha256": sha("crates/x/src/lib.rs")},
        });
        let receipt_path = root.join("verify/receipts/receipt.json");
        std::fs::write(&receipt_path, receipt.to_string()).unwrap();
        Fixture { _dir: dir, root, receipt_path, receipt }
    }

    #[test]
    fn valid_ooptdd_receipt_passes() {
        let f = make_ooptdd_fixture();
        let report = verify_receipt(&f.receipt_path, &f.root).unwrap();
        assert!(report.ok(), "failures: {:?}", report.checks.iter().filter(|c| !c.ok).collect::<Vec<_>>());
    }

    #[test]
    fn tampered_hash_fails() {
        let mut f = make_ooptdd_fixture();
        f.receipt["positive"]["receipt_sha256"] = json!("0".repeat(64));
        std::fs::write(&f.receipt_path, f.receipt.to_string()).unwrap();
        let report = verify_receipt(&f.receipt_path, &f.root).unwrap();
        assert!(!report.ok());
        assert!(report.checks.iter().any(|c| !c.ok && c.label.contains("positive.receipt_path")));
    }

    #[test]
    fn drifted_file_fails() {
        let f = make_ooptdd_fixture();
        write_file(&f.root, "crates/x/src/lib.rs", b"pub fn sync() { /* drifted */ }");
        let report = verify_receipt(&f.receipt_path, &f.root).unwrap();
        assert!(!report.ok());
        assert!(report.checks.iter().any(|c| !c.ok && c.label.contains("source_binding.path")));
    }

    #[test]
    fn missing_referenced_file_fails() {
        let f = make_ooptdd_fixture();
        std::fs::remove_file(f.root.join("verify/receipts/neg.log")).unwrap();
        let report = verify_receipt(&f.receipt_path, &f.root).unwrap();
        assert!(!report.ok());
        assert!(report.checks.iter().any(|c| !c.ok && c.label.contains("negative_oracle.receipt_path")));
    }

    #[test]
    fn missing_required_field_fails() {
        let mut f = make_ooptdd_fixture();
        f.receipt.as_object_mut().unwrap().remove("requirements");
        std::fs::write(&f.receipt_path, f.receipt.to_string()).unwrap();
        let report = verify_receipt(&f.receipt_path, &f.root).unwrap();
        assert!(!report.ok());
    }

    #[test]
    fn incoherent_verdict_fails() {
        let mut f = make_ooptdd_fixture();
        f.receipt["negative_oracle"]["observed_verdict"] = json!("green");
        std::fs::write(&f.receipt_path, f.receipt.to_string()).unwrap();
        let report = verify_receipt(&f.receipt_path, &f.root).unwrap();
        assert!(!report.ok());
    }

    #[test]
    fn template_only_receipt_fails() {
        let mut f = make_ooptdd_fixture();
        f.receipt["template_only"] = json!(true);
        std::fs::write(&f.receipt_path, f.receipt.to_string()).unwrap();
        let report = verify_receipt(&f.receipt_path, &f.root).unwrap();
        assert!(!report.ok());
    }

    #[test]
    fn unknown_schema_is_usage_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.json");
        std::fs::write(&p, json!({"schema_version": "mystery/v9"}).to_string()).unwrap();
        assert!(verify_receipt(&p, dir.path()).is_err());
    }

    #[test]
    fn missing_receipt_file_is_usage_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(verify_receipt(&dir.path().join("nope.json"), dir.path()).is_err());
    }

    #[test]
    fn pi_cycle_receipt_validates_and_tamper_fails() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        write_file(&root, "verify/receipts/measurement.json", b"ooptdd receipt bytes");
        let sha = sha256_hex(&root.join("verify/receipts/measurement.json")).unwrap();
        let receipt = json!({
            "schema_version": "pi-cycle/v1",
            "template_only": false,
            "cycle_id": "pi-x",
            "repo": {"git_head": "deadbeef"},
            "coordination": {"claim_status": "HELD"},
            "measurement": {"receipt_path": "verify/receipts/measurement.json", "receipt_sha256": sha},
        });
        let p = root.join("cycle.json");
        std::fs::write(&p, receipt.to_string()).unwrap();
        assert!(verify_receipt(&p, &root).unwrap().ok());
        // Tamper the pinned measurement file -> the binding must fire.
        write_file(&root, "verify/receipts/measurement.json", b"tampered");
        let report = verify_receipt(&p, &root).unwrap();
        assert!(!report.ok());
        assert!(report.checks.iter().any(|c| !c.ok && c.label.contains("measurement.receipt_path")));
    }

    /// A mini git repo: `lib.rs` committed with content A at rev1, rewritten to
    /// B at rev2. The receipt pins sha256(A) (+ producer.git_head=rev1 unless
    /// `with_git_head` is false) while disk carries B.
    struct GitFixture {
        _dir: tempfile::TempDir,
        root: std::path::PathBuf,
        receipt_path: std::path::PathBuf,
        receipt: Value,
        rev1: String,
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn sha256_bytes(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    const CONTENT_A: &[u8] = b"pub fn sync() { /* rev1 */ }";
    const CONTENT_B: &[u8] = b"pub fn sync() { /* rev2: evolved after sealing */ }";

    fn make_git_fixture(with_git_head: bool) -> GitFixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        git(&root, &["init", "-q"]);
        write_file(&root, "crates/x/tests/spec.rs", b"the locked spec");
        write_file(&root, "verify/receipts/pos.log", b"4 passed; 0 failed");
        write_file(&root, "verify/receipts/neg.log", b"1 failed (injected)");
        write_file(&root, "crates/x/src/lib.rs", CONTENT_A);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "rev1"]);
        let rev1 = git(&root, &["rev-parse", "HEAD"]);
        write_file(&root, "crates/x/src/lib.rs", CONTENT_B);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "rev2: disk evolved"]);
        let sha = |rel: &str| sha256_hex(&root.join(rel)).unwrap();
        let producer = if with_git_head {
            json!({"command": "cargo test -p x", "exit_code": 0, "git_head": rev1})
        } else {
            json!({"command": "cargo test -p x", "exit_code": 0})
        };
        let receipt = json!({
            "schema_version": "symposium-ooptdd-receipt/v1",
            "template_only": false,
            "receipt_id": "x-receipt",
            "cycle_id": "test-cycle",
            "requirement_group": "X",
            "spec": {"path": "crates/x/tests/spec.rs", "sha256": sha("crates/x/tests/spec.rs")},
            "producer": producer,
            "correlation": {"cid": "x-cid"},
            "requirements": [{"id": "R1", "role": "guard_mechanism", "event": "e"}],
            "positive": {
                "observed_verdict": "green",
                "receipt_path": "verify/receipts/pos.log",
                "receipt_sha256": sha("verify/receipts/pos.log"),
            },
            "negative_oracle": {
                "observed_verdict": "red",
                "receipt_path": "verify/receipts/neg.log",
                "receipt_sha256": sha("verify/receipts/neg.log"),
                "restored": true,
            },
            "source_binding": {"path": "crates/x/src/lib.rs", "symbol": "X::sync", "sha256": sha256_bytes(CONTENT_A)},
        });
        let receipt_path = root.join("verify/receipts/receipt.json");
        std::fs::write(&receipt_path, receipt.to_string()).unwrap();
        GitFixture { _dir: dir, root, receipt_path, receipt, rev1 }
    }

    #[test]
    fn sealed_consistent_when_disk_evolved_passes() {
        let f = make_git_fixture(true);
        let report = verify_receipt(&f.receipt_path, &f.root).unwrap();
        assert!(report.ok(), "failures: {:?}", report.checks.iter().filter(|c| !c.ok).collect::<Vec<_>>());
        let binding = report.checks.iter().find(|c| c.label.contains("source_binding.path")).unwrap();
        assert!(binding.ok);
        assert!(binding.label.contains("sealed-consistent"), "label: {}", binding.label);
        assert!(binding.label.contains(&f.rev1[..8]), "label: {}", binding.label);
    }

    #[test]
    fn garbage_sha_with_pinned_rev_fails() {
        let mut f = make_git_fixture(true);
        f.receipt["source_binding"]["sha256"] = json!("0".repeat(64));
        std::fs::write(&f.receipt_path, f.receipt.to_string()).unwrap();
        let report = verify_receipt(&f.receipt_path, &f.root).unwrap();
        assert!(!report.ok());
        let binding = report.checks.iter().find(|c| c.label.contains("source_binding.path")).unwrap();
        assert!(!binding.ok);
        assert!(binding.detail.contains("provenance hole"), "detail: {}", binding.detail);
    }

    #[test]
    fn pinned_rev_absent_fails() {
        let mut f = make_git_fixture(true);
        f.receipt["producer"]["git_head"] = json!("1".repeat(40));
        std::fs::write(&f.receipt_path, f.receipt.to_string()).unwrap();
        let report = verify_receipt(&f.receipt_path, &f.root).unwrap();
        assert!(!report.ok());
        let binding = report.checks.iter().find(|c| c.label.contains("source_binding.path")).unwrap();
        assert!(!binding.ok);
        assert!(binding.detail.contains("not in history"), "detail: {}", binding.detail);
    }

    #[test]
    fn no_git_head_keeps_legacy_fail() {
        let f = make_git_fixture(false);
        let report = verify_receipt(&f.receipt_path, &f.root).unwrap();
        assert!(!report.ok());
        let binding = report.checks.iter().find(|c| c.label.contains("source_binding.path")).unwrap();
        assert!(!binding.ok);
        assert!(binding.detail.contains("tampered or drifted"), "detail: {}", binding.detail);
    }
}
