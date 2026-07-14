// KG: multi-process transfer333 node smoke — 4 TCP authorities + 1 submit.
//
// Spawns the real `node` binary as OS processes over loopback TCP, waits for
// `certified` from submit and `cert_applied` (bob=30) from each authority.
// Marked `#[ignore]` (slow process spawn); run with:
//   cargo test --test node_smoke -- --ignored --nocapture

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
#[ignore = "multi-process TCP smoke; run with --ignored"]
fn four_authorities_one_submit_certifies_and_applies() {
    let ports = free_ports(5); // a0..a3 + client
    let auth_ports = &ports[0..4];
    let client_port = ports[4];

    let auth_addrs: Vec<String> = auth_ports
        .iter()
        .map(|p| format!("127.0.0.1:{p}"))
        .collect();
    let peers_csv = auth_addrs.join(",");
    let committee = "a0=0,a1=1,a2=2,a3=3";
    let genesis = "alice=100,bob=0";

    let mut authorities: Vec<ChildProc> = Vec::new();
    for i in 0..4 {
        let id = format!("a{i}");
        let listen = auth_addrs[i].clone();
        let args = vec![
            "authority".into(),
            "--id".into(),
            id.clone(),
            "--seed".into(),
            format!("{i}"),
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

    let submit_args = vec![
        "submit".into(),
        "--seed".into(),
        "99".into(),
        "--listen".into(),
        format!("127.0.0.1:{client_port}"),
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
    let mut submit = ChildProc::spawn("submit", &submit_args);

    let deadline = Instant::now() + Duration::from_secs(30);
    let certified = wait_until(deadline, || {
        submit
            .snapshot()
            .iter()
            .any(|l| l.contains("\"event\":\"certified\""))
    });
    assert!(
        certified,
        "submit never emitted certified; lines={:?}",
        submit.snapshot()
    );

    let applied = wait_until(deadline, || {
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
    eprintln!("=== submit lines ===");
    for l in submit.snapshot() {
        eprintln!("{l}");
    }
    for a in &authorities {
        eprintln!("=== {} lines ===", a.name);
        for l in a.snapshot() {
            eprintln!("{l}");
        }
    }

    submit.kill();
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
