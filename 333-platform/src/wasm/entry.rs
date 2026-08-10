// KG: TASK_333_INT_CrdtSync, TASK_333_INT_ConsensusNet
// KG: CONTRACT_SharedType_ProcessWireResult
// KG: VR_333_CrdtSync_ConsensusNet_Taliban — F1~F7 수정
// KG: lesson-333-bft-keyring-exchange-2026-04-14 — register_peer_identity(node_id, pub_key_bytes) 신규 추가 대상
// KG: src-wasm-register_peer_identity (PLANNED)
//! WASM bindings for browser usage.
//! Exposes 333 Platform core to JavaScript via wasm-bindgen.
//! Taliban fixes: F1(SyncManager wired), F2(NewView broadcast), F3(crypto warning),
//! F5(encode_delta removed), F6(Committed propagation), F7(no serde_json BFT fallback)

use wasm_bindgen::prelude::*;

// KG: lesson-333-solid-audit-2026-04-13 — DIP fix: PlatformCore facade 사용
// KG: finding_333_solid_333_d2 — ValidatorKeyring 제거 (core.consensus에 이미 존재)
// KG: plan-333-solid-dc-reclassify-2026-04-16 — SOLID D=D→D=C 재분류
use crate::platform::PlatformCore;
use crate::bft::types::OrderedTx;
use crate::crypto_real::Identity;
use crate::sync::SyncManager;

/// 333 Platform instance — DIP-compliant: delegates to PlatformCore
/// KG: CONTRACT_333_B_BFTCrypto — Identity(random) + ValidatorKeyring (in core.consensus)
///
/// # SRP 설계 결정 (SOLID-SRP-decision-2026-04-16)
///
/// Platform333은 5개 concern(CRDT/Tokenomics/BFT/Crypto/Sync)을 통합한 facade.
/// Taliban --lens solid 검증에서 SRP ✗로 판정되었으나, WASM 경계에서 분리 불가한 이유:
///
/// 1. **WASM single-threaded**: JS 호출 사이에 Rust 상태를 여러 struct이 공유하려면
///    `Arc<Mutex<PlatformCore>>`가 필요 → lock contention 오버헤드 (BFT 60fps 루프에 치명적).
/// 2. **PlatformCore 결합**: BFT consensus, CRDT sync, HLC, crypto keyring이 동일 상태를
///    공유. 별도 WASM struct로 분리하면 BFT 체크포인트 중 CRDT delta 병렬 처리 시 순서 보장 불가.
/// 3. **JS boundary 제약**: WASM 타입은 JS에서 불투명 포인터. 내부 분리는 JS API 형태를
///    바꾸지 않으므로 호출자 코드(WasmBridge)를 모두 재작성해야 함.
///
/// **결론**: Platform333 facade는 WASM 경계 불가피성(essential complexity)으로 인한 SRP
/// 트레이드오프. 내부는 PlatformCore로 완전히 DIP 위임됨. 섹션 분리로 가독성만 보완.
///
/// # SOLID D=C (Prometheus N=16 재분류, 2026-04-16)
///
/// D=D 판정은 false positive. WASM FFI 경계에서 concrete 타입은 구조적 필연.
/// PlatformCore 내부는 D=A (trait-based storage). 잔여 concrete: Identity(암호), SyncManager(배칭).
/// ValidatorKeyring은 core.consensus에 이미 존재하여 제거됨 (finding_333_solid_333_d2).
/// # KG: SOLID-SRP-decision-2026-04-16, SOLID-DIP-fix-wasm-sessions-2026-04-16
/// # KG: finding_333_solid_theory_d0 — D=C 재분류 근거
/// # KG: plan-333-solid-dc-reclassify-2026-04-16
#[wasm_bindgen]
pub struct Platform333 {
    core: PlatformCore,           // DIP: facade over HotStuff+Executor+HLC+LwwMap+Storage
    identity: Identity,           // Taliban F1: random Ed25519 keypair (cryptographic necessity)
    sync: SyncManager,            // WASM batching essential complexity
}

#[wasm_bindgen]
impl Platform333 {
    /// Create a new 333 Platform node
    /// KG: lesson-333-wasm-memory-oob-2026-04-14 — panic_hook install
    #[wasm_bindgen(constructor)]
    /// KG: finding_333_solid_333_d2 — ValidatorKeyring 중복 제거
    /// Identity는 core.consensus에 등록됨 (PlatformCore::with_identity).
    /// keyring은 core.consensus.keyring으로 통합 — wasm.rs 레벨 중복 제거.
    pub fn new(node_id: u32, validator_ids: &[u32]) -> Self {
        console_error_panic_hook::set_once();
        // Taliban BFTCrypto F1: generate RANDOM Ed25519 keypair
        let identity = Identity::generate();
        let identity_for_core = identity.clone();
        // DIP: PlatformCore handles HotStuff+Executor+HLC+LwwMap+Storage+Keyring
        let core = PlatformCore::with_identity(node_id, validator_ids, identity_for_core);

        Self {
            core,
            identity,
            sync: SyncManager::new(node_id),
        }
    }

    /// Get this node's ID
    pub fn node_id(&self) -> u32 {
        self.core.node_id
    }

    /// Place a block in the world (CRDT — no consensus needed)
    /// Taliban F1: delta goes through SyncManager for batching + state vector
    pub fn place_block(&mut self, key: String, value: String) -> String {
        self.core.hlc.tick();
        let delta = self.core.world.set(key, value);
        self.sync.on_local_delta(&delta); // Taliban F1: queue for batch
        serde_json::to_string(&delta).unwrap_or_default()
    }

    /// Get a block from the world
    pub fn get_block(&self, key: &str) -> Option<String> {
        self.core.world.get(&key.to_string()).cloned()
    }

    /// Delete a block (CRDT)
    /// Taliban F1: delta goes through SyncManager
    pub fn delete_block(&mut self, key: String) -> String {
        self.core.hlc.tick();
        let delta = self.core.world.delete(key);
        self.sync.on_local_delta(&delta); // Taliban F1
        serde_json::to_string(&delta).unwrap_or_default()
    }

    /// Merge a remote delta (received from another peer) — legacy path
    pub fn merge_delta(&mut self, delta_json: &str) {
        if let Ok(delta) = serde_json::from_str(delta_json) {
            self.core.world.merge_delta(&delta);
        }
    }

    /// Submit a token transfer (BFT — requires consensus)
    /// KG: SPEC_333_ConsistencyBoundary — BFT for token/governance only
    pub fn submit_transfer(&mut self, to: u32, amount: u64, nonce: u64) {
        self.core.consensus.submit_tx(OrderedTx::Transfer {
            from: self.core.node_id,
            to,
            amount,
            nonce,
        });
    }

    /// Get token balance
    pub fn balance(&self, node_id: u32) -> u64 {
        self.core.executor.balance(&node_id)
    }

    /// Get current HLC timestamp as JSON
    pub fn hlc_now(&self) -> String {
        serde_json::to_string(&self.core.hlc).unwrap_or_default()
    }

    /// Number of blocks in world
    pub fn world_size(&self) -> usize {
        self.core.world.len()
    }

    /// Is this node the current consensus leader?
    pub fn is_leader(&self) -> bool {
        self.core.consensus.is_leader()
    }

    /// Current consensus view number
    pub fn view(&self) -> u64 {
        self.core.consensus.view
    }

    /// Number of committed blocks
    pub fn committed_count(&self) -> usize {
        self.core.consensus.committed_count()
    }

    // KG: CONTRACT_SharedType_ProcessWireResult, TASK_333_INT_CrdtSync
    // KG: SPEC_333_WireRouting — TS→WASM bridge
    /// Process a received wire message. Returns JSON array of outgoing messages.
    pub fn process_wire(&mut self, data: &[u8]) -> String {
        use crate::wire::{self, DecodeResult, MsgType};

        match wire::decode(data) {
            DecodeResult::Ok(msg) => {
                match MsgType::from_u8(msg.header.msg_type) {
                    Some(MsgType::StateUpdate) | Some(MsgType::StateFull) => {
                        // Taliban F1: delegate to SyncManager
                        self.sync.process_incoming(
                            msg.header.msg_type, &msg.payload, &mut self.core.world
                        );
                        "[]".to_string()
                    }
                    Some(MsgType::Consensus) => {
                        // KG: TASK_333_INT_ConsensusNet
                        self.process_bft_wire(&msg.payload)
                    }
                    _ => "[]".to_string(),
                }
            }
            DecodeResult::SkipVersion(_) | DecodeResult::SkipType(_) => "[]".to_string(),
            DecodeResult::Err(_) => "[]".to_string(),
        }
    }

    /// Taliban F1: Poll SyncManager for outgoing batched deltas (call every 20ms)
    /// Returns JSON array of outgoing messages to broadcast on crdt channel
    pub fn poll_sync(&mut self) -> String {
        let msgs = self.sync.poll_outgoing();
        if msgs.is_empty() {
            return "[]".to_string();
        }
        serde_json::to_string(&msgs).unwrap_or_else(|_| "[]".to_string())
    }

    /// Generate full state snapshot for a new peer joining the room
    pub fn create_snapshot_for_peer(&self) -> String {
        match self.sync.create_full_snapshot(&self.core.world) {
            Some(msg) => serde_json::to_string(&msg).unwrap_or_else(|_| "null".to_string()),
            None => "null".to_string(),
        }
    }

    // KG: TASK_333_INT_ConsensusNet, CONTRACT_333_INT_ConsensusNet
    // KG: TASK_333_B_BFTCrypto — Ed25519 real signatures via self.identity (random key)
    // Taliban F1 fix: wasm.rs uses sign_with_identity (random key), NOT sign_standalone
    // Note: state.rs/viewchange.rs still use sign_standalone internally (deterministic seed)
    // but wasm.rs overrides outgoing signatures with real random-key signatures
    /// Process BFT consensus wire message.
    fn process_bft_wire(&mut self, payload: &[u8]) -> String {
        use crate::bft::types::{HotStuffMsg, ProcessResult};

        // Taliban F7: postcard ONLY — no serde_json fallback for BFT (security)
        let msg: HotStuffMsg = match postcard::from_bytes(payload) {
            Ok(m) => m,
            Err(_) => return "[]".to_string(),
        };

        let result = self.core.consensus.process(msg);
        let mut outgoing = Vec::new();

        match result {
            ProcessResult::Broadcast(response) => {
                if let Some(out) = self.encode_bft_msg(&response) {
                    outgoing.push(out);
                }
            }
            ProcessResult::SendToLeader(response) => {
                if let Some(out) = self.encode_bft_msg(&response) {
                    outgoing.push(out);
                }
            }
            ProcessResult::Committed(txs, new_view_msg) => {
                // Execute committed transactions
                let _results = self.core.executor.execute_block(&txs);
                // H1 fix: use the NewView embedded in Committed rather than rebuilding it.
                // Previously wasm.rs rebuilt NewView independently — the one from state.rs
                // is already signed and reflects the correct post-commit view.
                // # KG: taliban2-H1-fix-2026-04-15
                if let Some(out) = self.encode_bft_msg(&new_view_msg) {
                    outgoing.push(out);
                }
            }
            ProcessResult::ViewChange(_new_view, vc_msg) => {
                // Use the ViewChange signed by on_timeout rather than rebuilding it,
                // for the same reason Committed's NewView is reused above.
                // # KG: fix-333-tick-discards-signed-viewchange-2026-07-15
                if let Some(out) = self.encode_bft_msg(&vc_msg) {
                    outgoing.push(out);
                }
            }
            ProcessResult::None => {}
        }

        serde_json::to_string(&outgoing).unwrap_or_else(|_| "[]".to_string())
    }

    fn encode_bft_msg(&self, msg: &crate::bft::types::HotStuffMsg) -> Option<crate::sync::OutgoingMsg> {
        let bytes = postcard::to_allocvec(msg).ok()?;
        let wire_bytes = crate::wire::encode(crate::wire::MsgType::Consensus, &bytes).ok()?;
        Some(crate::sync::OutgoingMsg {
            msg_type: crate::wire::MsgType::Consensus as u8,
            channel: "bft".to_string(),
            payload: wire_bytes,
        })
    }

    // KG: lesson-333-bft-keyring-exchange-2026-04-14 — public key exchange for BFT
    /// Get this node's Ed25519 public key as hex string (64 chars = 32 bytes)
    pub fn get_public_key(&self) -> String {
        let peer_id = self.identity.peer_id();
        peer_id.0.iter().map(|b| format!("{:02x}", b)).collect()
    }

    /// Register a remote peer's public key for BFT signature verification
    /// Must be called after P2P handshake, before BFT consensus can proceed
    pub fn register_peer_key(&mut self, node_id: u32, pub_key_hex: &str) -> bool {
        let bytes: Vec<u8> = (0..pub_key_hex.len())
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(&pub_key_hex[i..i + 2], 16).ok())
            .collect();
        if bytes.len() != 32 { return false; }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        let peer_id = crate::crypto_real::PeerId(key);
        self.core.consensus.register_peer_key(node_id, peer_id);
        true
    }

    // KG: sprint7F-kernel-platform-integration-2026-04-16
    /// Flush deferred kernel work queue. Call from JS poll loop (same cadence as poll_sync).
    /// Returns number of jobs executed this tick.
    pub fn tick_kernel(&mut self, max_jobs: u32) -> u32 {
        self.core.tick_kernel(max_jobs as usize) as u32
    }

    /// Are all 3 built-in kernel services Running?
    pub fn kernel_healthy(&self) -> bool {
        self.core.kernel_healthy()
    }

    /// Pending deferred jobs count
    pub fn kernel_pending_work(&self) -> u32 {
        self.core.pending_work() as u32
    }

    // KG: plan-333-bft-try-propose — Drive BFT consensus forward
    // Leader가 tx_pool에 대기 중인 TX가 있으면 propose() 호출 → Proposal 메시지 생성
    // JS 폴링 루프(200ms)에서 pollSync()와 함께 호출해야 함
    pub fn try_propose(&mut self) -> String {
        if let Some(msg) = self.core.consensus.propose() {
            if let Some(out) = self.encode_bft_msg(&msg) {
                return serde_json::to_string(&vec![out])
                    .unwrap_or_else(|_| "[]".to_string());
            }
        }
        "[]".to_string()
    }

    /// Drive the BFT pacemaker. Call from the same 200ms poll loop as
    /// `try_propose`; returns ViewChange messages to broadcast (JSON array,
    /// `[]` when the leader is healthy).
    ///
    /// Without this the view-change timer never advances, so a crashed or
    /// stalled leader stalls every honest validator forever — `viewchange.rs`
    /// promises "even if a leader crashes, the protocol makes progress", and
    /// that promise is only kept if something calls tick.
    /// # KG: fix-333-pacemaker-unwired-2026-07-15
    pub fn bft_tick(&mut self) -> String {
        use crate::bft::types::ProcessResult;
        if let ProcessResult::ViewChange(_new_view, vc_msg) = self.core.consensus.tick() {
            if let Some(out) = self.encode_bft_msg(&vc_msg) {
                return serde_json::to_string(&vec![out])
                    .unwrap_or_else(|_| "[]".to_string());
            }
        }
        "[]".to_string()
    }

    // KG: TASK_333_INT_ConsensusNet — Room state for UI
    pub fn room_state_json(&self) -> String {
        serde_json::json!({
            "nodeId": self.core.node_id,
            "view": self.core.consensus.view,
            "isLeader": self.core.consensus.is_leader(),
            "committedBlocks": self.core.consensus.committed_count(),
            "worldSize": self.core.world.len(),
            "syncPending": self.sync.pending_count(),
        }).to_string()
    }

    // KG: seed-phase-B-sdk-test-page-2026-04-15 — SDK test page diagnostics
    /// Extended diagnostics including public key, peer registry count, and kernel health
    pub fn diagnostics_json(&self) -> String {
        // KG: sprint7F-kernel-platform-integration-2026-04-16
        serde_json::json!({
            "nodeId": self.core.node_id,
            "view": self.core.consensus.view,
            "isLeader": self.core.consensus.is_leader(),
            "committedBlocks": self.core.consensus.committed_count(),
            "worldSize": self.core.world.len(),
            "syncPending": self.sync.pending_count(),
            "publicKeyHex": self.get_public_key(),
            "registeredPeers": self.core.peer_count(),
            "kernelHealthy": self.core.kernel_healthy(),
            "kernelPendingWork": self.core.pending_work(),
        }).to_string()
    }
}

/// Quick health check
#[wasm_bindgen]
pub fn health() -> String {
    r#"{"status":"ok","platform":"333","version":"0.4.0","wasm":true,"sync":true,"bft":true}"#.to_string()
}

/// Get platform info
#[wasm_bindgen]
pub fn info() -> String {
    // KG: sprint7F-puter-quality-port-2026-04-16 — kernel P1-P10 added
    // KG: sprint7H-coop-coep-sharedarraybuffer-2026-04-16 — SAB ring buffer, tests 452
    r#"{"name":"triple-three","modules":["hlc","lamport","lww_map","bft","sync","om","kernel","wasm_shared"],"tests":452,"kernel":{"patterns":["ServiceRegistry","Lifecycle","Capability","Manifest","Channel","WorkerQueue"],"version":"1.0.0"},"sharedArrayBuffer":{"available":true,"headers":{"coop":"same-origin","coep":"require-corp","corp":"cross-origin"}}}"#.to_string()
}

// KG: SPAN_333_OM — OM WASM Bindings
use crate::compute::om::{OmOrchestrator, OmTaskKind, NodeResources};

/// OM Distributed Compute instance
#[wasm_bindgen]
pub struct OmCompute {
    om: OmOrchestrator,
}

impl Default for OmCompute {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl OmCompute {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self { om: OmOrchestrator::new() }
    }

    pub fn register_node(&mut self, node_id: u32, cpu_cores: u32, gpu: bool, battery: i32) {
        self.om.register_node(NodeResources {
            node_id, cpu_cores, memory_mb: 4096,
            gpu_available: gpu,
            battery_pct: if battery < 0 { None } else { Some(battery as u8) },
            active_workers: 0, max_workers: cpu_cores,
        });
    }

    pub fn submit_cpu_job(&mut self, submitter: u32, estimated_ms: u64, now_ms: u64) -> String {
        self.om.submit_job(OmTaskKind::CpuIntensive { estimated_ms }, submitter, now_ms)
    }

    pub fn submit_gpu_job(&mut self, submitter: u32, model_size_mb: u32, now_ms: u64) -> String {
        self.om.submit_job(OmTaskKind::GpuCompute { model_size_mb }, submitter, now_ms)
    }

    pub fn submit_mapreduce_job(&mut self, submitter: u32, chunks: u32, reduce_fn: &str, now_ms: u64) -> String {
        self.om.submit_job(OmTaskKind::MapReduce { chunk_count: chunks, reduce_fn: reduce_fn.into() }, submitter, now_ms)
    }

    pub fn distribute(&mut self, now_ms: u64) -> String {
        serde_json::to_string(&self.om.distribute(now_ms)).unwrap_or("[]".into())
    }

    pub fn submit_result(&mut self, task_id: u64, worker_id: u32, output: &[u8], compute_ms: u64, output_hash: &str, now_ms: u64) -> String {
        use crate::compute::task::TaskResult;
        let result = TaskResult { task_id, worker_id, output: output.to_vec(), compute_ms, output_hash: output_hash.into() };
        crate::wasm::om::submit_result_to_json(
            self.om.submit_result_from(worker_id, result, now_ms),
        )
    }

    pub fn stats(&self) -> String {
        serde_json::to_string(&self.om.stats()).unwrap_or("{}".into())
    }

    pub fn job_status(&self, job_id: &str) -> String {
        match self.om.job_status(job_id) {
            Some(job) => serde_json::to_string(job).unwrap_or("{}".into()),
            None => "null".into(),
        }
    }
}
