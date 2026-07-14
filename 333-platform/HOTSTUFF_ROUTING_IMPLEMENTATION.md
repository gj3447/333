# HotStuff Routing Implementation Guide — 333 Platform

> **Companion to**: HOTSTUFF_ROUTING_PATTERNS.md  
> **Target**: INT_ConsensusNet (Integration Phase, AtomicSpan)  
> **Date**: 2026-04-13  
> **KG**: TASK_HotStuff_Routing_Implementation, SPAN_333_Integration  
> **Status**: Implementation Ready (Pseudocode → Rust)

---

## Architecture: 4-Layer Stack

```
┌─────────────────────────────────────────────┐
│  Consensus Layer (HotStuffState)            │  ← Generates HotStuffMsg
│  - Process proposals, votes                 │
│  - Track phase progress                     │
│  - Trigger leader rotation                  │
└─────────────────────────────────────────────┘
              ↓ encode(&msg)
┌─────────────────────────────────────────────┐
│  Transport Abstraction (Transport trait)    │  ← Routes to/from network
│  - send(to, msg): direct unicast             │
│  - broadcast(msg): all-except-self           │
│  - recv(): poll incoming messages            │
└─────────────────────────────────────────────┘
         ↓ encode binary           ↑ decode binary
┌─────────────────────────────────────────────┐
│  Network Layer (MeshRoom)                   │  ← WebRTC peer management
│  - peers: HashMap<u32, PeerInfo>            │
│  - channels: HashMap<u32, DataChannel>      │
│  - send_to(peer_id): direct or fallback     │
│  - broadcast(): parallel sends               │
└─────────────────────────────────────────────┘
              ↓ write to DataChannel
┌─────────────────────────────────────────────┐
│  WebRTC Layer                               │  ← Browser WebRTC API
│  - RtcDataChannel (ordered: true)           │
│  - RtcPeerConnection                        │
└─────────────────────────────────────────────┘
```

---

## 1. Update Transport Trait: Add Leader Awareness

**File**: `src/bft/transport.rs`

```rust
/// Transport trait — with leader-aware routing
pub trait Transport {
    /// Send a message to a specific validator (unicast)
    fn send(&mut self, to: NodeId, msg: HotStuffMsg) -> Result<(), TransportError>;

    /// Broadcast a message to all validators except self
    fn broadcast(&mut self, msg: HotStuffMsg) -> Result<(), TransportError>;

    /// Receive next message (non-blocking). Returns (sender, message).
    fn recv(&mut self) -> Option<(NodeId, HotStuffMsg)>;
    
    // NEW: Leader-aware routing
    
    /// Send directly to leader (with fallback to broadcast)
    fn send_to_leader(&mut self, leader_id: NodeId, msg: HotStuffMsg) -> Result<(), TransportError> {
        // Try direct send first
        match self.send(leader_id, msg.clone()) {
            Ok(_) => Ok(()),
            Err(_) => {
                // Fallback: broadcast (any peer may relay)
                eprintln!("[WARN] Direct send to leader {} failed, broadcasting", leader_id);
                self.broadcast(msg)
            }
        }
    }
    
    /// Check if we have a direct connection to a peer
    fn is_connected(&self, peer_id: NodeId) -> bool;
    
    /// Get list of connected peers
    fn connected_peers(&self) -> Vec<NodeId>;
    
    /// Update internal view of current leader (for optimization)
    fn update_leader(&mut self, leader_id: NodeId);
}

#[derive(Debug, Clone)]
pub enum TransportError {
    NoPeer,           // peer_id not found
    SendFailed,       // underlying send failed
    NotConnected,     // no connection to peer
}
```

---

## 2. Implement WebRTC Transport

**File**: `src/p2p/webrtc_transport.rs` (NEW)

```rust
// KG: SPAN_333_WebRTC_Transport
use crate::bft::transport::{Transport, TransportError};
use crate::bft::types::HotStuffMsg;
use crate::p2p::mesh::MeshRoom;
use std::collections::VecDeque;

/// WebRTC-backed transport for HotStuff
pub struct WebRtcTransport {
    mesh: MeshRoom,
    current_leader: u32,
    
    // Message queues per phase (prevent head-of-line blocking)
    proposal_queue: VecDeque<(u32, HotStuffMsg)>,  // from leader only
    vote_queues: [VecDeque<(u32, HotStuffMsg)>; 3],  // [Prepare, PreCommit, Commit]
    newview_queue: VecDeque<(u32, HotStuffMsg)>,
}

impl WebRtcTransport {
    pub fn new(mesh: MeshRoom, initial_leader: u32) -> Self {
        Self {
            mesh,
            current_leader: initial_leader,
            proposal_queue: VecDeque::new(),
            vote_queues: Default::default(),
            newview_queue: VecDeque::new(),
        }
    }
}

impl Transport for WebRtcTransport {
    fn send(&mut self, to: u32, msg: HotStuffMsg) -> Result<(), TransportError> {
        // Encode message to binary
        let encoded = serde_json::to_vec(&msg)
            .map_err(|_| TransportError::SendFailed)?;
        
        // Use mesh to route message
        self.mesh.send_to(to, &encoded, ChannelMode::Reliable)
            .then(|| Ok(()))
            .ok_or(TransportError::NoPeer)
    }
    
    fn broadcast(&mut self, msg: HotStuffMsg) -> Result<(), TransportError> {
        let encoded = serde_json::to_vec(&msg)
            .map_err(|_| TransportError::SendFailed)?;
        
        let sent = self.mesh.broadcast(&encoded, ChannelMode::Reliable);
        if sent > 0 {
            Ok(())
        } else {
            Err(TransportError::SendFailed)
        }
    }
    
    fn recv(&mut self) -> Option<(u32, HotStuffMsg)> {
        // Poll mesh for incoming messages
        let events = self.mesh.poll();
        
        for event in events {
            use crate::p2p::mesh::MeshEvent;
            match event {
                MeshEvent::MessageReceived { from, data } => {
                    // Decode binary message
                    if let Ok(msg) = serde_json::from_slice::<HotStuffMsg>(&data) {
                        // Route to appropriate queue based on message type
                        match &msg {
                            HotStuffMsg::Proposal { .. } => {
                                self.proposal_queue.push_back((from, msg.clone()));
                            }
                            HotStuffMsg::Vote { phase, .. } => {
                                let phase_idx = match phase {
                                    crate::bft::types::Phase::Prepare => 0,
                                    crate::bft::types::Phase::PreCommit => 1,
                                    crate::bft::types::Phase::Commit => 2,
                                    crate::bft::types::Phase::Decide => 0, // shouldn't happen
                                };
                                self.vote_queues[phase_idx].push_back((from, msg.clone()));
                            }
                            HotStuffMsg::NewView { .. } => {
                                self.newview_queue.push_back((from, msg.clone()));
                            }
                            HotStuffMsg::ViewChange { .. } => {
                                // Treat as broadcast-equivalent
                                self.newview_queue.push_back((from, msg.clone()));
                            }
                        }
                    }
                }
                MeshEvent::PeerJoined(peer_id) => {
                    eprintln!("[INFO] Peer {} joined mesh", peer_id);
                }
                MeshEvent::PeerLeft(peer_id) => {
                    eprintln!("[WARN] Peer {} left mesh", peer_id);
                }
                MeshEvent::PeerTimedOut(peer_id) => {
                    eprintln!("[WARN] Peer {} timed out", peer_id);
                }
                MeshEvent::RoomFull => {
                    eprintln!("[WARN] Mesh room is full");
                }
                _ => {}
            }
        }
        
        // Drain queues in priority order
        self.proposal_queue.pop_front()
            .or_else(|| self.newview_queue.pop_front())
            .or_else(|| self.vote_queues[0].pop_front())
            .or_else(|| self.vote_queues[1].pop_front())
            .or_else(|| self.vote_queues[2].pop_front())
    }
    
    fn is_connected(&self, peer_id: u32) -> bool {
        self.mesh.peer_ids().contains(&peer_id)
    }
    
    fn connected_peers(&self) -> Vec<u32> {
        self.mesh.peer_ids()
    }
    
    fn update_leader(&mut self, leader_id: u32) {
        self.current_leader = leader_id;
    }
}
```

---

## 3. Integrate with Consensus State Machine

**File**: `src/bft/state.rs` — Modify `process()` method

```rust
impl HotStuffState {
    /// Process an incoming message and return actions
    pub fn process(&mut self, msg: HotStuffMsg) -> ProcessResult {
        use crate::bft::types::{HotStuffMsg::*, ProcessResult::*};
        
        match msg {
            Proposal { block, phase, signature } => {
                // Validate proposal
                if block.view != self.view || phase != self.phase {
                    return None;  // Ignore out-of-phase proposals
                }
                
                let proposer = block.proposer;
                if proposer != self.current_leader() {
                    return None;  // Ignore proposals from non-leaders
                }
                
                // Verify signature
                if !verify_signature(&signature, proposer, block.hash) {
                    return None;  // Invalid signature
                }
                
                // Store block
                self.blocks.insert(block.hash, block.clone());
                self.pending_block = Some(block.clone());
                
                // Create vote
                let vote = HotStuffMsg::Vote {
                    block_hash: block.hash,
                    view: self.view,
                    phase,
                    signature: sign(self.node_id, block.hash),
                };
                
                // ← KEY: Route vote to current leader
                SendToLeader(vote)
            }
            
            Vote { block_hash, view, phase, signature } => {
                // Ignore votes from old views
                if view != self.view || phase != self.phase {
                    return None;
                }
                
                // Prevent equivocation
                if let Some(prev_hash) = self.vote_tracker.get(&(self.node_id, phase)) {
                    if *prev_hash != block_hash {
                        eprintln!("[ERROR] Equivocation detected!");
                        return None;
                    }
                }
                
                // If I'm the leader, collect this vote
                if self.is_leader() {
                    self.votes.entry(block_hash)
                        .or_insert_with(Vec::new)
                        .push(signature);
                    
                    let vote_count = self.votes[&block_hash].len();
                    
                    // Check if we have quorum
                    if vote_count >= self.quorum_size() {
                        // Form QC and broadcast NewView
                        let qc = QuorumCert {
                            block_hash,
                            signers: vec![],  // simplified
                            // In production: include aggregate signature
                        };
                        
                        // Advance to next phase
                        self.phase = self.next_phase();
                        
                        let newview = HotStuffMsg::NewView {
                            view: self.view,
                            qc,
                            signature: sign(self.node_id, block_hash),
                        };
                        
                        return Broadcast(newview);
                    }
                }
                
                None
            }
            
            NewView { view, qc, .. } => {
                if view != self.view || view <= self.high_qc.view {
                    return None;  // Stale or invalid
                }
                
                self.high_qc = qc;
                
                // Advance phase
                self.phase = self.next_phase();
                
                // If I'm the leader and have transactions, propose
                if self.is_leader() && !self.tx_pool.is_empty() {
                    if let Some(proposal) = self.propose() {
                        return Broadcast(proposal);
                    }
                }
                
                None
            }
            
            ViewChange { new_view, high_qc, .. } => {
                if new_view <= self.view {
                    return None;  // Stale view change
                }
                
                // Transition to new view
                let old_leader = self.current_leader();
                self.view = new_view;
                self.phase = Phase::Prepare;
                self.high_qc = high_qc;
                self.pending_block = None;
                self.votes.clear();
                
                let new_leader = self.current_leader();
                eprintln!("[INFO] View change: {} → {}, leader {} → {}", 
                          self.view - 1, self.view, old_leader, new_leader);
                
                // If I'm the new leader, propose immediately
                if self.is_leader() && !self.tx_pool.is_empty() {
                    if let Some(proposal) = self.propose() {
                        return Broadcast(proposal);
                    }
                }
                
                None
            }
        }
    }
}
```

---

## 4. Main Event Loop: Integrating Consensus + Transport

**File**: `src/bft/consensus_engine.rs` (NEW)

```rust
// KG: SPAN_333_ConsensusEngine
use crate::bft::transport::Transport;
use crate::bft::types::ProcessResult;
use std::time::{Duration, Instant};

pub struct ConsensusEngine {
    state: HotStuffState,
    transport: Box<dyn Transport>,
    
    // View change timeout
    view_start_time: Instant,
    view_timeout: Duration,
    
    // Transaction pool
    pending_txs: Vec<OrderedTx>,
}

impl ConsensusEngine {
    pub fn new(
        state: HotStuffState,
        transport: Box<dyn Transport>,
        view_timeout_ms: u64,
    ) -> Self {
        Self {
            state,
            transport,
            view_start_time: Instant::now(),
            view_timeout: Duration::from_millis(view_timeout_ms),
            pending_txs: Vec::new(),
        }
    }
    
    /// Process all pending messages and return committed blocks
    pub fn run_step(&mut self) -> Vec<Vec<OrderedTx>> {
        let mut committed_blocks = Vec::new();
        
        // Step 1: Poll transport for incoming messages
        while let Some((sender, msg)) = self.transport.recv() {
            eprintln!("[RECV] From {}: {:?}", sender, msg);
            
            let result = self.state.process(msg);
            self.apply_result(result);
        }
        
        // Step 2: Check for view timeout
        if self.view_start_time.elapsed() > self.view_timeout {
            eprintln!("[TIMEOUT] View {} timed out after {:?}", 
                      self.state.view, self.view_timeout);
            self.trigger_view_change();
        }
        
        // Step 3: If I'm leader and have pending transactions, propose
        if self.state.is_leader() && !self.pending_txs.is_empty() {
            self.state.submit_txs(self.pending_txs.drain(..).collect());
            if let Some(proposal) = self.state.propose() {
                let _ = self.transport.broadcast(proposal);
            }
        }
        
        // Step 4: Extract committed blocks for execution
        committed_blocks = self.state.committed
            .drain(..)
            .map(|block| block.transactions)
            .collect();
        
        committed_blocks
    }
    
    fn apply_result(&mut self, result: ProcessResult) {
        use crate::bft::types::ProcessResult::*;
        match result {
            SendToLeader(msg) => {
                let leader = self.state.current_leader();
                let _ = self.transport.send_to_leader(leader, msg);
            }
            Broadcast(msg) => {
                let _ = self.transport.broadcast(msg);
            }
            ViewChange(new_view) => {
                self.state.view = new_view;
                self.view_start_time = Instant::now();
            }
            Committed(_) => {
                // Blocks are automatically moved to self.state.committed
            }
            None => {}
        }
    }
    
    fn trigger_view_change(&mut self) {
        let old_view = self.state.view;
        self.state.view += 1;
        self.state.phase = Phase::Prepare;
        self.view_start_time = Instant::now();
        
        eprintln!("[VIEW_CHANGE] {} → {}", old_view, self.state.view);
        
        // Notify transport of new leader
        let new_leader = self.state.current_leader();
        self.transport.update_leader(new_leader);
        
        // Broadcast view change to trigger other validators
        let viewchange = HotStuffMsg::ViewChange {
            new_view: self.state.view,
            sender: self.state.node_id,
            high_qc: self.state.high_qc.clone(),
            signature: sign(self.state.node_id, self.state.view),
        };
        let _ = self.transport.broadcast(viewchange);
    }
    
    pub fn submit_tx(&mut self, tx: OrderedTx) -> bool {
        if self.pending_txs.len() >= 10_000 {
            return false;  // Pool full
        }
        self.pending_txs.push(tx);
        true
    }
}
```

---

## 5. WASM Wrapper: Expose Consensus to JavaScript

**File**: `src/wasm.rs` — Add consensus routing

```rust
// KG: SPAN_333_WASM_Consensus
#[wasm_bindgen]
impl Platform333 {
    // Existing methods: place_block, delete_block, etc.
    
    // NEW: Consensus routing
    
    /// Submit a transaction requiring consensus
    #[wasm_bindgen]
    pub fn transfer(&mut self, to: u32, amount: u64) {
        let nonce = self.consensus.next_nonce();  // prevent replay
        self.consensus.submit_tx(OrderedTx::Transfer {
            from: self.node_id,
            to,
            amount,
            nonce,
        });
    }
    
    /// Process one round of consensus (call this in requestAnimationFrame)
    #[wasm_bindgen]
    pub fn consensus_step(&mut self) -> String {
        let committed = self.consensus_engine.run_step();
        
        // Execute committed transactions
        for block_txs in committed {
            for tx in block_txs {
                match tx {
                    OrderedTx::Transfer { from, to, amount, .. } => {
                        self.executor.transfer(from, to, amount).ok();
                    }
                    _ => {}
                }
            }
        }
        
        // Return consensus status as JSON
        serde_json::json!({
            "view": self.consensus.state.view,
            "phase": format!("{:?}", self.consensus.state.phase),
            "is_leader": self.consensus.state.is_leader(),
            "committed_count": self.consensus.state.committed_count(),
        }).to_string()
    }
    
    /// Get current view and leader
    #[wasm_bindgen]
    pub fn consensus_status(&self) -> String {
        serde_json::json!({
            "view": self.consensus.state.view,
            "leader": self.consensus.state.current_leader(),
            "is_leader": self.consensus.state.is_leader(),
        }).to_string()
    }
}
```

---

## 6. Testing: 4-Node Integration Test

**File**: `tests/hotstuff_mesh_integration.rs` (NEW)

```rust
#[test]
fn four_node_hotstuff_consensus() {
    // Setup: 4 validators with InMemory network
    let validators = vec![1, 2, 3, 4];
    let mut networks = InMemoryNetwork::create(&validators);
    
    let mut states = validators.iter()
        .map(|&id| HotStuffState::new(id, ValidatorSet::new(validators.clone())))
        .collect::<Vec<_>>();
    
    // Node 1 is initial leader
    assert_eq!(states[0].current_leader(), 1);
    
    // Step 1: Leader proposes block
    let proposal = states[0].propose().unwrap();
    networks[0].broadcast(proposal);
    
    // Step 2: Validators receive proposal and vote
    let msg = networks[1].recv().unwrap();
    let vote = states[1].process(msg.1).unwrap();
    networks[1].send_to(1, vote);
    
    let msg = networks[2].recv().unwrap();
    let vote = states[2].process(msg.1).unwrap();
    networks[2].send_to(1, vote);
    
    let msg = networks[3].recv().unwrap();
    let vote = states[3].process(msg.1).unwrap();
    networks[3].send_to(1, vote);
    
    // Step 3: Leader collects votes
    while let Some((_, msg)) = networks[0].recv() {
        states[0].process(msg);
    }
    
    // Leader should have formed QC
    assert!(states[0].votes[0].len() >= 3);  // quorum
    
    println!("✓ 4-node consensus: proposal → votes → quorum");
}
```

---

## Summary: Integration Roadmap

| File | Change | Lines | Est. Time |
|------|--------|-------|-----------|
| `src/bft/transport.rs` | Add `send_to_leader()`, leader methods | +20 | 15 min |
| `src/p2p/webrtc_transport.rs` | NEW: WebRTC Transport impl | +150 | 2 hours |
| `src/bft/state.rs` | Modify `process()` for routing | +40 | 1 hour |
| `src/bft/consensus_engine.rs` | NEW: Main event loop | +120 | 2 hours |
| `src/wasm.rs` | Add consensus_step, transfer | +30 | 30 min |
| `tests/hotstuff_mesh_integration.rs` | NEW: 4-node test | +80 | 1 hour |

**Total: ~7 hours implementation, 2 hours debugging/testing**

---

## KG References

- HOTSTUFF_ROUTING_PATTERNS.md (this research)
- SPAN_333_Integration (parent integration task)
- ATOM_WebRTC_Memory_Research (WebRTC cleanup prerequisite)
- INT_MemFix, INT_CrdtSync (dependencies)

