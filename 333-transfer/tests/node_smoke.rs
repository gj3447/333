// KG: multi-process transfer333 node smoke — 4 TCP authorities + 1 submit.
//
// Spawns the real `node` binary as OS processes over loopback TCP. A forged
// Alice order arrives first and must receive zero votes; the legitimate Alice
// key then submits the same sequence slot and must still certify/apply. This is
// the process-level falsifier for owner-auth-before-slot-lock ordering.
// This runs in the default test suite so the process boundary cannot silently
// regress while unit-only checks remain green.

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn free_ports(n: usize) -> Vec<u16> {
    // Bind all first so we don't accidentally re-pick a just-dropped port.
    let listeners: Vec<TcpListener> = (0..n)
        .map(|_| TcpListener::bind("127.0.0.1:0").expect("bind ephemeral"))
        .collect();
    let ports: Vec<u16> = listeners
        .iter()
        .map(|l| l.local_addr().unwrap().port())
        .collect();
    drop(listeners);
    ports
}

struct ChildProc {
    name: String,
    child: Child,
    lines: Arc<Mutex<Vec<String>>>,
}

impl ChildProc {
    fn spawn(name: &str, args: &[String]) -> Self {
        let bin = env!("CARGO_BIN_EXE_node");
        let mut child = Command::new(bin)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {name}: {e}"));

        let stdout = child.stdout.take().expect("stdout");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let lines_t = Arc::clone(&lines);
        let name_t = name.to_string();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        let l = l.trim().to_string();
                        if l.is_empty() {
                            continue;
                        }
                        eprintln!("[{name_t}] {l}");
                        lines_t.lock().expect("lines").push(l);
                    }
                    Err(_) => break,
                }
            }
        });

        // Drain stderr so the child never blocks on a full pipe.
        if let Some(stderr) = child.stderr.take() {
            let name_e = name.to_string();
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().flatten() {
                    eprintln!("[{name_e}:err] {line}");
                }
            });
        }

        Self {
            name: name.to_string(),
            child,
            lines,
        }
    }

    fn snapshot(&self) -> Vec<String> {
        self.lines.lock().expect("lines").clone()
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ChildProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn wait_until(deadline: Instant, mut pred: impl FnMut() -> bool) -> bool {
    while Instant::now() < deadline {
        if pred() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    pred()
}

#[test]
fn forged_owner_first_does_not_poison_valid_same_slot() {
    let ports = free_ports(6); // a0..a3 + forged client + valid client
    let auth_ports = &ports[0..4];
    let forged_client_port = ports[4];
    let valid_client_port = ports[5];

    let auth_addrs: Vec<String> = auth_ports
        .iter()
        .map(|p| format!("127.0.0.1:{p}"))
        .collect();
    let peers_csv = auth_addrs.join(",");
    // Public-key-only rosters. Private material is supplied separately through
    // the explicitly test-only --dev-seed path (production uses --key-file).
    let committee = concat!(
        "a0=3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29,",
        "a1=8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c,",
        "a2=8139770ea87d175f56a35466c34c7ecccb8d8a91b4ee37a25df60f5b8fc9b394,",
        "a3=ed4928c628d1c2c6eae90338905995612959273a5c63f93636c14614ac8737d1"
    );
    let owner_roster = concat!(
        "alice=197f6b23e16c8532c6abc838facd5ea789be0c76b2920334039bfa8b3d368d61,",
        "bob=4508a07aa941707f3eb2db94c8897a80b2c1197476b6de213ac273df7d86c4ff"
    );
    let network_id = "transfer333-node-smoke-v2";
    let genesis = "alice=100,bob=0";

    let mut authorities: Vec<ChildProc> = Vec::new();
    for i in 0..4 {
        let id = format!("a{i}");
        let listen = auth_addrs[i].clone();
        let args = vec![
            "authority".into(),
            "--id".into(),
            id.clone(),
            "--dev-seed".into(),
            format!("{i}"),
            "--network-id".into(),
            network_id.into(),
            "--owner-roster".into(),
            owner_roster.into(),
            "--listen".into(),
            listen,
            "--peers".into(),
            peers_csv.clone(),
            "--committee".into(),
            committee.into(),
            "--genesis".into(),
            genesis.into(),
            // Stay alive long enough for submit + apply; harness kills at end.
            "--rounds-idle-exit".into(),
            "5000".into(),
        ];
        authorities.push(ChildProc::spawn(&id, &args));
    }

    // Wait for all authorities to report committee_ready.
    let deadline = Instant::now() + Duration::from_secs(15);
    let ready = wait_until(deadline, || {
        authorities.iter().all(|a| {
            a.snapshot()
                .iter()
                .any(|l| l.contains("\"event\":\"committee_ready\""))
        })
    });
    assert!(
        ready,
        "authorities did not become ready; lines: {:?}",
        authorities
            .iter()
            .map(|a| (a.name.clone(), a.snapshot()))
            .collect::<Vec<_>>()
    );

    // Forge first: seed 99 is not Alice's registered key (seed 42).
    let forged_args = vec![
        "submit".into(),
        "--dev-seed".into(),
        "99".into(),
        "--network-id".into(),
        network_id.into(),
        "--owner-roster".into(),
        owner_roster.into(),
        "--listen".into(),
        format!("127.0.0.1:{forged_client_port}"),
        "--peers".into(),
        peers_csv.clone(),
        "--committee".into(),
        committee.into(),
        "--transfer".into(),
        "alice:0:bob:30".into(),
        "--max-rounds".into(),
        "200".into(),
        "--pause-ms".into(),
        "10".into(),
    ];
    let mut forged = ChildProc::spawn("forged-submit", &forged_args);

    let deadline = Instant::now() + Duration::from_secs(30);
    let forged_failed = wait_until(deadline, || {
        forged
            .snapshot()
            .iter()
            .any(|l| l.contains("\"event\":\"cert_failed\""))
    });
    assert!(
        forged_failed,
        "forged submit did not fail certification; lines={:?}",
        forged.snapshot()
    );

    let rejected_by_all = wait_until(deadline, || {
        authorities.iter().all(|a| {
            a.snapshot().iter().any(|l| {
                l.contains("\"event\":\"owner_auth_rejected\"")
                    && l.contains("\"transfer\":\"alice:0:bob:30\"")
                    && l.contains("\"reason\":\"invalid_owner_signature\"")
            })
        })
    });
    assert!(
        rejected_by_all,
        "not every authority rejected forged Alice; lines={:?}",
        authorities
            .iter()
            .map(|a| (a.name.clone(), a.snapshot()))
            .collect::<Vec<_>>()
    );
    assert!(
        authorities.iter().all(|a| !a.snapshot().iter().any(|l| {
            l.contains("\"event\":\"vote_cast\"")
                && l.contains("\"transfer\":\"alice:0:bob:30\"")
        })),
        "forged order must receive zero authority votes"
    );
    assert!(
        authorities.iter().all(|a| !a
            .snapshot()
            .iter()
            .any(|l| l.contains("\"event\":\"cert_applied\""))),
        "forged order must not mutate a ledger"
    );
    forged.kill();

    // The legitimate Alice owner (seed 42) submits the exact same sequence slot.
    // Success proves the forged order did not occupy the authority slot lock.
    let valid_args = vec![
        "submit".into(),
        "--dev-seed".into(),
        "42".into(),
        "--network-id".into(),
        network_id.into(),
        "--owner-roster".into(),
        owner_roster.into(),
        "--listen".into(),
        format!("127.0.0.1:{valid_client_port}"),
        "--peers".into(),
        peers_csv,
        "--committee".into(),
        committee.into(),
        "--transfer".into(),
        "alice:0:bob:30".into(),
        "--max-rounds".into(),
        "200".into(),
        "--pause-ms".into(),
        "10".into(),
    ];
    let mut valid = ChildProc::spawn("valid-submit", &valid_args);

    let valid_deadline = Instant::now() + Duration::from_secs(30);
    let certified = wait_until(valid_deadline, || {
        valid
            .snapshot()
            .iter()
            .any(|l| l.contains("\"event\":\"certified\""))
    });
    assert!(
        certified,
        "valid Alice did not certify after forgery; lines={:?}",
        valid.snapshot()
    );

    let applied = wait_until(valid_deadline, || {
        authorities.iter().all(|a| {
            a.snapshot().iter().any(|l| {
                l.contains("\"event\":\"cert_applied\"") && l.contains("\"bob\":30")
            })
        })
    });
    assert!(
        applied,
        "not all authorities applied cert with bob=30; auth lines={:?}",
        authorities
            .iter()
            .map(|a| (a.name.clone(), a.snapshot()))
            .collect::<Vec<_>>()
    );

    // Capture observed JSON for the final report.
    eprintln!("=== forged submit lines ===");
    for l in forged.snapshot() {
        eprintln!("{l}");
    }
    eprintln!("=== valid submit lines ===");
    for l in valid.snapshot() {
        eprintln!("{l}");
    }
    for a in &authorities {
        eprintln!("=== {} lines ===", a.name);
        for l in a.snapshot() {
            eprintln!("{l}");
        }
    }

    valid.kill();
    for a in &mut authorities {
        a.kill();
    }
}

#[test]
fn bad_args_print_usage_and_exit_nonzero() {
    let bin = env!("CARGO_BIN_EXE_node");
    let out = Command::new(bin)
        .arg("authority")
        .arg("--help")
        .output()
        .expect("run node");
    assert!(
        !out.status.success(),
        "expected non-zero exit for bad args, got {:?}",
        out.status
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("usage:") || err.contains("missing required"),
        "stderr should mention usage, got: {err}"
    );
}
