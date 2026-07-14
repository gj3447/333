// KG: sprint7E-sdk-decouple-2026-04-16, lesson-333-sdk-test-coupling-2026-04-14
//! SDK Integration Tests — direct Rust-level testing without browser/room UI.
//!
//! Decouples SDK validation from browser harness (full-test.mjs / p2p-4tab-test.mjs).
//! Tests SchemaRegistry, EventBus, and PlatformCore independently.

use triple_three::sdk::schema::{SchemaRegistry, CrdtType, SchemaError};
use triple_three::sdk::events::{EventBus, EventType, Event};
use triple_three::platform::{PlatformCore, ExecutionRequest, ExecutionResponse};
use std::sync::{Arc, atomic::{AtomicU32, Ordering}};

// ── SchemaRegistry SDK Tests ─────────────────────────────────────────

#[test]
fn sdk_register_all_four_apps() {
    let mut reg = SchemaRegistry::new();
    for app in &["rts", "social", "editor", "om"] {
        reg.register_app(app).unwrap();
    }
    assert_eq!(reg.apps().len(), 4);
}

#[test]
fn sdk_rts_schema_full() {
    let mut reg = SchemaRegistry::new();
    reg.register_app("rts").unwrap();
    reg.register_field("rts", "units", CrdtType::LwwMap, "Unit positions and state").unwrap();
    reg.register_field("rts", "resources", CrdtType::LwwMap, "Resource node state").unwrap();
    reg.register_field("rts", "chat", CrdtType::RGA, "In-game chat").unwrap();
    reg.register_field("rts", "players", CrdtType::ORSet, "Active players").unwrap();

    let schema = reg.get_schema("rts").unwrap();
    assert_eq!(schema.fields.len(), 4);
    assert_eq!(reg.field_type("rts", "units"), Some(CrdtType::LwwMap));
    assert_eq!(reg.field_type("rts", "players"), Some(CrdtType::ORSet));
}

#[test]
fn sdk_social_schema() {
    let mut reg = SchemaRegistry::new();
    reg.register_app("social").unwrap();
    reg.register_field("social", "profiles", CrdtType::LwwMap, "User profiles").unwrap();
    reg.register_field("social", "followers", CrdtType::ORSet, "Follower sets").unwrap();
    reg.register_field("social", "posts", CrdtType::RGA, "Post feed").unwrap();

    assert_eq!(reg.field_type("social", "profiles"), Some(CrdtType::LwwMap));
    assert_eq!(reg.field_type("social", "followers"), Some(CrdtType::ORSet));
}

#[test]
fn sdk_cross_app_isolation() {
    let mut reg = SchemaRegistry::new();
    reg.register_app("app_a").unwrap();
    reg.register_app("app_b").unwrap();
    reg.register_field("app_a", "data", CrdtType::LwwMap, "").unwrap();

    // app_b doesn't have "data"
    assert_eq!(reg.field_type("app_b", "data"), None);
    // app_a does
    assert_eq!(reg.field_type("app_a", "data"), Some(CrdtType::LwwMap));
}

// ── EventBus SDK Tests ───────────────────────────────────────────────

#[test]
fn sdk_event_lifecycle() {
    let mut bus = EventBus::new();
    let counter = Arc::new(AtomicU32::new(0));

    // Subscribe to multiple event types
    let c1 = Arc::clone(&counter);
    let _id1 = bus.on(EventType::PeerJoined, move |_| { c1.fetch_add(1, Ordering::SeqCst); });
    let c2 = Arc::clone(&counter);
    let _id2 = bus.on(EventType::StateChanged, move |_| { c2.fetch_add(10, Ordering::SeqCst); });

    // Emit peer joined
    bus.emit(Event { event_type: EventType::PeerJoined, data: vec![], source: Some(1), timestamp_ms: 100 });
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // Emit state changed
    bus.emit(Event { event_type: EventType::StateChanged, data: b"delta".to_vec(), source: None, timestamp_ms: 200 });
    assert_eq!(counter.load(Ordering::SeqCst), 11);
}

#[test]
fn sdk_custom_events_for_apps() {
    let mut bus = EventBus::new();
    let rts_counter = Arc::new(AtomicU32::new(0));
    let social_counter = Arc::new(AtomicU32::new(0));

    let rc = Arc::clone(&rts_counter);
    bus.on(EventType::Custom("rts:unit_spawned".into()), move |_| { rc.fetch_add(1, Ordering::SeqCst); });
    let sc = Arc::clone(&social_counter);
    bus.on(EventType::Custom("social:post_created".into()), move |_| { sc.fetch_add(1, Ordering::SeqCst); });

    bus.emit(Event { event_type: EventType::Custom("rts:unit_spawned".into()), data: vec![], source: None, timestamp_ms: 0 });
    bus.emit(Event { event_type: EventType::Custom("social:post_created".into()), data: vec![], source: None, timestamp_ms: 0 });

    assert_eq!(rts_counter.load(Ordering::SeqCst), 1);
    assert_eq!(social_counter.load(Ordering::SeqCst), 1);
}

#[test]
fn sdk_event_log_for_late_joiner() {
    let mut bus = EventBus::new();
    // Emit 10 events before "late joiner" subscribes
    for i in 0..10 {
        bus.emit(Event {
            event_type: EventType::StateChanged,
            data: vec![i as u8],
            source: None,
            timestamp_ms: i as u64 * 100,
        });
    }
    // Late joiner can see last 5 events
    let recent = bus.recent_events(5);
    assert_eq!(recent.len(), 5);
    assert_eq!(recent[0].data[0], 5); // event 5 through 9
}

// ── PlatformCore SDK Tests ───────────────────────────────────────────

#[test]
fn sdk_platform_crdt_round_trip() {
    let mut p1 = PlatformCore::new(1, &[1, 2, 3, 4]);
    let mut p2 = PlatformCore::new(2, &[1, 2, 3, 4]);

    // Player 1 places a block
    let resp = p1.execute(ExecutionRequest::CrdtUpdate {
        key: "5,5".into(),
        value: Some("stone".into()),
    });
    // Extract delta and send to player 2
    if let ExecutionResponse::CrdtDelta(json) = resp {
        p2.merge_remote_delta(&json);
    }
    // Both should have the block
    assert_eq!(p1.get_block("5,5"), Some(&"stone".to_string()));
    assert_eq!(p2.get_block("5,5"), Some(&"stone".to_string()));
}

#[test]
fn sdk_platform_bft_submit() {
    let mut p = PlatformCore::new(1, &[1, 2, 3, 4]);
    let resp = p.execute(ExecutionRequest::Ordered(
        triple_three::bft::types::OrderedTx::Transfer { from: 1, to: 2, amount: 100, nonce: 0 }
    ));
    assert!(matches!(resp, ExecutionResponse::Queued));
}

#[test]
fn sdk_platform_token_transfer() {
    let mut p = PlatformCore::new(1, &[1, 2, 3, 4]);
    let resp = p.execute(ExecutionRequest::Token(
        triple_three::tokenomics::TokenTx::Transfer { from: 1, to: 2, amount: 500, nonce: 0 }
    ));
    assert!(matches!(resp, ExecutionResponse::TokenResult(triple_three::tokenomics::TokenResult::Success)));
    assert_eq!(p.balance(1), 10000 - 500);
    assert_eq!(p.balance(2), 10000 + 500);
}

#[test]
fn sdk_multi_peer_crdt_convergence() {
    // 4 peers all modify different keys, then merge
    let validators = &[1, 2, 3, 4];
    let mut peers: Vec<PlatformCore> = validators.iter()
        .map(|&id| PlatformCore::new(id, validators))
        .collect();

    // Each peer writes a unique key
    let mut deltas = Vec::new();
    for (i, peer) in peers.iter_mut().enumerate() {
        let resp = peer.execute(ExecutionRequest::CrdtUpdate {
            key: format!("key_{}", i),
            value: Some(format!("val_{}", i)),
        });
        if let ExecutionResponse::CrdtDelta(json) = resp {
            deltas.push(json);
        }
    }

    // All peers merge all deltas
    for delta in &deltas {
        for peer in &mut peers {
            peer.merge_remote_delta(delta);
        }
    }

    // All peers should have all 4 keys
    for peer in &peers {
        assert_eq!(peer.world_size(), 4, "all peers must converge to 4 keys");
        for i in 0..4 {
            assert_eq!(
                peer.get_block(&format!("key_{}", i)),
                Some(&format!("val_{}", i)),
                "peer must have key_{}", i
            );
        }
    }
}
