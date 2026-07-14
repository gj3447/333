// KG: sprint7I-dc-transport-tdd-2026-04-16
//! Transport compliance tests — properties that ANY BftTransport implementation must satisfy.
//! Runs against both MpscTransport and DataChannelTransport to validate trait contracts.

use triple_three::bft::crypto::{QuorumCert, Signature};
use triple_three::bft::types::{HotStuffMsg, Phase};
use triple_three::netcode::transport::{BftTransport, make_mpsc_transports};
use triple_three::netcode::dc_transport::make_dc_transports;

// ── Test helpers ──────────────────────────────────────────────────────────────

fn make_newview(view: u64, signer: u32) -> HotStuffMsg {
    HotStuffMsg::NewView {
        view,
        qc: QuorumCert::genesis(),
        signature: Signature { signer, sig_bytes: vec![0u8; 64] },
    }
}

fn make_vote(view: u64, signer: u32) -> HotStuffMsg {
    HotStuffMsg::Vote {
        view,
        block_hash: 0xCAFE,
        phase: Phase::Prepare,
        signature: Signature { signer, sig_bytes: vec![0u8; 64] },
    }
}

// ── Compliance macro — same tests, different transport ────────────────────────

macro_rules! transport_compliance {
    ($mod_name:ident, $make_fn:expr) => {
        mod $mod_name {
            use super::*;

            #[test]
            fn broadcast_reaches_all_peers() {
                let ids = [1u32, 2, 3, 4];
                let mut transports = $make_fn(&ids);
                let msg = make_newview(1, 1);
                transports[0].broadcast(msg);

                // All peers (not sender for DC, including sender for Mpsc) should receive
                let mut total_received = 0;
                for t in &mut transports[1..] {
                    total_received += t.poll_incoming().len();
                }
                assert!(total_received >= 3, "at least 3 peers must receive broadcast, got {}", total_received);
            }

            #[test]
            fn point_to_point_only_reaches_target() {
                let ids = [1u32, 2, 3];
                let mut transports = $make_fn(&ids);
                let vote = make_vote(1, 2);
                transports[1].send_to_leader(1, vote);

                // Node 1 receives
                assert_eq!(transports[0].poll_incoming().len(), 1);
                // Node 3 does NOT receive
                assert_eq!(transports[2].poll_incoming().len(), 0);
            }

            #[test]
            fn empty_poll_returns_empty_vec() {
                let ids = [1u32, 2];
                let mut transports = $make_fn(&ids);
                assert!(transports[0].poll_incoming().is_empty());
                assert!(transports[1].poll_incoming().is_empty());
            }

            #[test]
            fn node_id_matches_creation_order() {
                let ids = [10u32, 20, 30];
                let transports = $make_fn(&ids);
                assert_eq!(transports[0].node_id(), 10);
                assert_eq!(transports[1].node_id(), 20);
                assert_eq!(transports[2].node_id(), 30);
            }

            #[test]
            fn multiple_messages_preserved_fifo() {
                let ids = [1u32, 2];
                let mut transports = $make_fn(&ids);

                for v in 1..=5u64 {
                    transports[0].broadcast(make_newview(v, 1));
                }

                let received = transports[1].poll_incoming();
                assert_eq!(received.len(), 5);
                for (i, msg) in received.iter().enumerate() {
                    match msg {
                        HotStuffMsg::NewView { view, .. } => {
                            assert_eq!(*view, (i + 1) as u64, "FIFO order violated");
                        }
                        _ => panic!("expected NewView"),
                    }
                }
            }

            #[test]
            fn bidirectional_communication() {
                let ids = [1u32, 2];
                let mut transports = $make_fn(&ids);

                transports[0].broadcast(make_newview(10, 1));
                transports[1].broadcast(make_newview(20, 2));

                let from_1 = transports[1].poll_incoming();
                let from_2 = transports[0].poll_incoming();

                assert!(!from_1.is_empty(), "node 2 must receive from node 1");
                assert!(!from_2.is_empty(), "node 1 must receive from node 2");
            }

            #[test]
            fn poll_drains_inbox() {
                let ids = [1u32, 2];
                let mut transports = $make_fn(&ids);

                transports[0].broadcast(make_newview(1, 1));
                let first = transports[1].poll_incoming();
                assert_eq!(first.len(), 1);

                // Second poll should be empty
                let second = transports[1].poll_incoming();
                assert!(second.is_empty(), "poll must drain: second poll should be empty");
            }

            #[test]
            fn send_to_self_as_leader() {
                let ids = [1u32, 2];
                let mut transports = $make_fn(&ids);

                // Node 1 sends vote to itself (it's the leader)
                let vote = make_vote(1, 1);
                transports[0].send_to_leader(1, vote);

                let received = transports[0].poll_incoming();
                // Must receive own message
                assert_eq!(received.len(), 1, "self-send must be received");
            }
        }
    }
}

// ── Instantiate for both transports ───────────────────────────────────────────

transport_compliance!(mpsc_compliance, |ids: &[u32]| {
    make_mpsc_transports(ids)
});

transport_compliance!(dc_compliance, |ids: &[u32]| {
    make_dc_transports(ids)
});
