// BFT Transport Layer Code Stubs
// KG: SPAN_333_BFT_Transport_CodeStubs
// Ready-to-implement modules for WebRTC-based HotStuff consensus

// ============================================================================
// MODULE 1: Routing Layer (src/bft/routing.rs)
// ============================================================================

// #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
// enum ConnectionRole {
//     LeaderIn,        // recv Proposals + NewView from leader
//     LeaderOut,       // send Votes to leader
//     GossipPeer,      // relay Votes, resync blocks
// }

// pub struct HotStuffRouter {
//     me: NodeId,
//     view: u64,
//     leader: NodeId,
//     validators: Vec<NodeId>,
//     connections: HashMap<ConnectionRole, Vec<NodeId>>,
//     inbox: Vec<(NodeId, HotStuffMsg)>,
//     outbox: Vec<(NodeId, HotStuffMsg)>,
// }

// impl HotStuffRouter {
//     pub fn new(me: NodeId, validators: Vec<NodeId>) -> Self {
//         let leader = leader_for_view(0, &validators);
//         Self {
//             me,
//             view: 0,
//             leader,
//             validators,
//             connections: HashMap::new(),
//             inbox: Vec::new(),
//             outbox: Vec::new(),
//         }
//     }

//     pub fn route_outgoing(&mut self, msg: HotStuffMsg) {
//         match &msg {
//             HotStuffMsg::Proposal { .. } => {
//                 if self.me == self.leader {
//                     for peer in self.all_peers() {
//                         self.outbox.push((peer, msg.clone()));
//                     }
//                 }
//             }
//             HotStuffMsg::Vote { .. } => {
//                 self.outbox.push((self.leader, msg));
//             }
//             HotStuffMsg::NewView { .. } => {
//                 if self.me == self.leader {
//                     for peer in self.all_peers() {
//                         self.outbox.push((peer, msg.clone()));
//                     }
//                 }
//             }
//             HotStuffMsg::ViewChange { new_view, .. } => {
//                 if let Some(new_leader) = leader_for_view(*new_view, &self.validators) {
//                     self.outbox.push((new_leader, msg));
//                 }
//                 // Also gossip to peers for reliability
//                 for peer in self.gossip_peers() {
//                     self.outbox.push((peer, msg.clone()));
//                 }
//             }
//         }
//     }

//     pub fn on_incoming(&mut self, from: NodeId, msg: HotStuffMsg) {
//         // Detect view change
//         if let HotStuffMsg::NewView { view, .. } = &msg {
//             if *view > self.view {
//                 self.view = *view;
//                 self.leader = leader_for_view(*view, &self.validators);
//             }
//         }
//         self.inbox.push((from, msg));
//     }

//     pub fn next_outgoing(&mut self) -> Option<(NodeId, HotStuffMsg)> {
//         self.outbox.pop()
//     }

//     pub fn next_incoming(&mut self) -> Option<(NodeId, HotStuffMsg)> {
//         self.inbox.pop()
//     }

//     pub fn all_peers(&self) -> Vec<NodeId> {
//         self.validators.iter()
//             .filter(|&&id| id != self.me)
//             .copied()
//             .collect()
//     }

//     pub fn gossip_peers(&self) -> Vec<NodeId> {
//         // Return 2-3 random peers for gossip (GossipSub style)
//         self.connections
//             .get(&ConnectionRole::GossipPeer)
//             .map(|peers| peers.clone())
//             .unwrap_or_default()
//     }

//     pub fn on_peer_joined(&mut self, peer_id: NodeId) {
//         // Assign role based on leader status
//         let role = if peer_id == self.leader {
//             ConnectionRole::LeaderIn
//         } else {
//             ConnectionRole::GossipPeer
//         };
//         self.connections.entry(role).or_insert_with(Vec::new).push(peer_id);
//     }

//     pub fn on_peer_left(&mut self, peer_id: NodeId) {
//         for peers in self.connections.values_mut() {
//             peers.retain(|&id| id != peer_id);
//         }
//     }

//     pub fn is_connected(&self, peer_id: NodeId) -> bool {
//         self.connections.values().any(|peers| peers.contains(&peer_id))
//     }

//     pub fn all_connected_peers(&self) -> Vec<NodeId> {
//         self.connections.values()
//             .flat_map(|peers| peers.iter().copied())
//             .collect::<std::collections::HashSet<_>>()
//             .into_iter()
//             .collect()
//     }
// }

// ============================================================================
// MODULE 2: View Synchronization (src/bft/view_sync.rs)
// ============================================================================

// use std::collections::HashMap;
// use super::crypto::NodeId;

// pub struct ViewSync {
//     my_view: u64,
//     heard_view: u64,
//     pending_acks: HashMap<u64, Vec<NodeId>>,
// }

// impl ViewSync {
//     pub fn new() -> Self {
//         Self {
//             my_view: 0,
//             heard_view: 0,
//             pending_acks: HashMap::new(),
//         }
//     }

//     pub fn on_new_view(&mut self, view: u64, _qc: &QuorumCert) {
//         if view > self.heard_view {
//             self.heard_view = view;
//         }
//     }

//     pub fn on_view_change_request(&mut self, new_view: u64, sender: NodeId) {
//         if new_view > self.heard_view {
//             self.heard_view = new_view;
//             self.pending_acks.entry(new_view)
//                 .or_insert_with(Vec::new)
//                 .push(sender);
//         }
//     }

//     pub fn should_advance(&self, f: usize) -> bool {
//         self.pending_acks.get(&self.heard_view)
//             .map_or(false, |acks| acks.len() > f)
//     }

//     pub fn advance(&mut self) -> Option<u64> {
//         if self.heard_view > self.my_view {
//             self.my_view = self.heard_view;
//             self.pending_acks.clear();
//             return Some(self.my_view);
//         }
//         None
//     }

//     pub fn force_catch_up(&self) -> Option<u64> {
//         if self.heard_view > self.my_view + 2 {
//             Some(self.heard_view)
//         } else {
//             None
//         }
//     }

//     pub fn current_view(&self) -> u64 {
//         self.my_view
//     }

//     pub fn heard_view(&self) -> u64 {
//         self.heard_view
//     }
// }

// ============================================================================
// MODULE 3: Adaptive Timeout (src/bft/timeout.rs)
// ============================================================================

// use std::collections::VecDeque;

// pub struct AdaptiveTimeout {
//     rtt_samples: VecDeque<u64>,
//     current_timeout_ms: u64,
// }

// impl AdaptiveTimeout {
//     pub fn new() -> Self {
//         Self {
//             rtt_samples: VecDeque::with_capacity(16),
//             current_timeout_ms: 500,
//         }
//     }

//     pub fn record_rtt(&mut self, rtt_ms: u64) {
//         self.rtt_samples.push_back(rtt_ms);
//         if self.rtt_samples.len() > 16 {
//             self.rtt_samples.pop_front();
//         }
//         self.update_timeout();
//     }

//     fn update_timeout(&mut self) {
//         if self.rtt_samples.is_empty() {
//             self.current_timeout_ms = 500;
//             return;
//         }
//         let mut sorted: Vec<_> = self.rtt_samples.iter().copied().collect();
//         sorted.sort_unstable();
//         let p95_idx = (sorted.len() * 95) / 100;
//         let p95_rtt = sorted[p95_idx.max(1) - 1];
//         self.current_timeout_ms = (p95_rtt * 3).max(200);
//     }

//     pub fn is_timeout(&self, elapsed_ms: u64) -> bool {
//         elapsed_ms > self.current_timeout_ms
//     }

//     pub fn current_timeout(&self) -> u64 {
//         self.current_timeout_ms
//     }
// }

// ============================================================================
// MODULE 4: WebRTC Transport Implementation (src/bft/webrtc_transport.rs)
// ============================================================================

// use std::rc::Rc;
// use std::cell::RefCell;
// use super::routing::HotStuffRouter;
// use super::view_sync::ViewSync;
// use super::timeout::AdaptiveTimeout;
// use super::transport::Transport;
// use super::types::HotStuffMsg;
// use super::crypto::NodeId;
// use crate::p2p::mesh::MeshRoom;

// pub struct WebRtcTransport {
//     mesh: Rc<RefCell<MeshRoom>>,
//     router: Rc<RefCell<HotStuffRouter>>,
//     view_sync: Rc<RefCell<ViewSync>>,
//     timeout: Rc<RefCell<AdaptiveTimeout>>,
//     last_leader_msg_ms: Rc<RefCell<u64>>,
//     local_id: NodeId,
// }

// impl WebRtcTransport {
//     pub fn new(
//         mesh: Rc<RefCell<MeshRoom>>,
//         router: Rc<RefCell<HotStuffRouter>>,
//         local_id: NodeId,
//     ) -> Self {
//         let now_ms = current_time_ms();
//         Self {
//             mesh,
//             router,
//             view_sync: Rc::new(RefCell::new(ViewSync::new())),
//             timeout: Rc::new(RefCell::new(AdaptiveTimeout::new())),
//             last_leader_msg_ms: Rc::new(RefCell::new(now_ms)),
//             local_id,
//         }
//     }

//     fn poll_mesh(&self) {
//         let mut mesh = self.mesh.borrow_mut();
//         let events: Vec<_> = mesh.drain_events().collect();
//
//         for event in events {
//             match event {
//                 crate::p2p::mesh::MeshEvent::MessageReceived { from, data } => {
//                     if let Ok(msg) = HotStuffMsg::from_bytes(&data) {
//                         *self.last_leader_msg_ms.borrow_mut() = current_time_ms();
//                         let mut router = self.router.borrow_mut();
//                         router.on_incoming(from, msg);
//                     }
//                 }
//                 crate::p2p::mesh::MeshEvent::PeerJoined(id) => {
//                     let mut router = self.router.borrow_mut();
//                     router.on_peer_joined(id);
//                 }
//                 crate::p2p::mesh::MeshEvent::PeerLeft(id) => {
//                     let mut router = self.router.borrow_mut();
//                     router.on_peer_left(id);
//                 }
//                 _ => {}
//             }
//         }
//     }

//     fn flush_outgoing(&self) {
//         let mut router = self.router.borrow_mut();
//         let mut mesh = self.mesh.borrow_mut();
//         while let Some((to, msg)) = router.next_outgoing() {
//             if let Ok(bytes) = msg.to_bytes() {
//                 let _ = mesh.send(to, &bytes);
//             }
//         }
//     }
// }

// impl Transport for WebRtcTransport {
//     fn send(&mut self, to: NodeId, msg: HotStuffMsg) {
//         let mut router = self.router.borrow_mut();
//         router.route_outgoing(msg);  // May queue multiple messages
//         drop(router);
//         self.flush_outgoing();
//     }

//     fn broadcast(&mut self, msg: HotStuffMsg) {
//         let mut router = self.router.borrow_mut();
//         for peer in router.all_peers() {
//             router.route_outgoing(msg.clone());
//         }
//         drop(router);
//         self.flush_outgoing();
//     }

//     fn recv(&mut self) -> Option<(NodeId, HotStuffMsg)> {
//         self.poll_mesh();
//         let mut router = self.router.borrow_mut();
//         router.next_incoming()
//     }

//     fn current_view(&self) -> u64 {
//         self.router.borrow().view
//     }

//     fn is_connected(&self, to: NodeId) -> bool {
//         self.router.borrow().is_connected(to)
//     }

//     fn validate_msg(&self, _from: NodeId, _msg: &HotStuffMsg) -> bool {
//         true  // TODO: Implement signature verification
//     }

//     fn is_leader_alive(&self, timeout_ms: u64) -> bool {
//         let last_msg_ms = *self.last_leader_msg_ms.borrow();
//         let now_ms = current_time_ms();
//         (now_ms - last_msg_ms) < timeout_ms
//     }

//     fn on_view_change(&mut self, new_view: u64) {
//         let msg = HotStuffMsg::ViewChange {
//             new_view,
//             sender: self.local_id,
//             high_qc: Default::default(),  // TODO: Get from state machine
//             signature: Default::default(),  // TODO: Sign properly
//         };
//         let mut router = self.router.borrow_mut();
//         router.route_outgoing(msg);
//         drop(router);
//         self.flush_outgoing();
//     }

//     fn on_peer_state_change(&mut self, peer_id: NodeId, connected: bool) {
//         let mut router = self.router.borrow_mut();
//         if connected {
//             router.on_peer_joined(peer_id);
//         } else {
//             router.on_peer_left(peer_id);
//         }
//     }

//     fn tick(&mut self, _now_ms: u64) {
//         self.poll_mesh();
//     }

//     fn close(&mut self) {
//         let mut mesh = self.mesh.borrow_mut();
//         mesh.close();
//     }
// }

// ============================================================================
// HELPER: Current Time in Milliseconds
// ============================================================================

// #[cfg(target_arch = "wasm32")]
// fn current_time_ms() -> u64 {
//     web_sys::window()
//         .and_then(|w| w.performance())
//         .map(|p| p.now() as u64)
//         .unwrap_or(0)
// }

// #[cfg(not(target_arch = "wasm32"))]
// fn current_time_ms() -> u64 {
//     std::time::SystemTime::now()
//         .duration_since(std::time::UNIX_EPOCH)
//         .map(|d| d.as_millis() as u64)
//         .unwrap_or(0)
// }

// ============================================================================
// INTEGRATION: Updated BFT Executor (src/bft/executor.rs usage)
// ============================================================================

// Example of how to integrate the transport into the BFT state machine:

// pub fn bft_main_loop(
//     state: &mut HotStuffState,
//     transport: &mut dyn Transport,
//     timeout_ms: u64,
// ) {
//     loop {
//         // 1. Check leader timeout
//         if !state.is_leader() && !transport.is_leader_alive(timeout_ms) {
//             // Initiate view change
//             let new_view = state.view + 1;
//             state.initiate_view_change(new_view);
//             transport.on_view_change(new_view);
//         }

//         // 2. Process incoming messages
//         while let Some((from, msg)) = transport.recv() {
//             if transport.validate_msg(from, &msg) {
//                 match state.process_message(from, &msg) {
//                     ProcessResult::Broadcast(out_msg) => {
//                         transport.broadcast(out_msg);
//                     }
//                     ProcessResult::SendToLeader(out_msg) => {
//                         let leader = state.current_leader();
//                         transport.send(leader, out_msg);
//                     }
//                     ProcessResult::Committed(txs) => {
//                         // Execute committed transactions
//                         execute_transactions(&txs);
//                     }
//                     ProcessResult::ViewChange(new_view) => {
//                         state.view = new_view;
//                         transport.on_view_change(new_view);
//                     }
//                     ProcessResult::None => {}
//                 }
//             }
//         }

//         // 3. If we're the leader, propose new block
//         if state.is_leader() && state.should_propose() {
//             if let Some(block) = state.create_block() {
//                 let msg = HotStuffMsg::Proposal {
//                     block,
//                     phase: state.phase,
//                     signature: sign_message(&msg),
//                 };
//                 transport.broadcast(msg);
//             }
//         }

//         // 4. Periodic maintenance
//         transport.tick(current_time_ms());

//         // 5. Yield to event loop
//         std::thread::sleep(std::time::Duration::from_millis(1));
//     }
// }

// ============================================================================
// TESTING: Unit Test Examples
// ============================================================================

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[test]
//     fn test_routing_proposal() {
//         let validators = vec![1, 2, 3, 4];
//         let mut router = HotStuffRouter::new(1, validators.clone());
//
//         let proposal = HotStuffMsg::Proposal {
//             block: Block::genesis(),
//             phase: Phase::Prepare,
//             signature: sign(1, 0),
//         };
//
//         router.route_outgoing(proposal.clone());
//
//         // Leader sends to all
//         assert_eq!(router.outbox.len(), 3);  // 4 validators - 1 self
//         let (to, _) = router.next_outgoing().unwrap();
//         assert!(validators.contains(&to));
//     }

//     #[test]
//     fn test_view_sync_catch_up() {
//         let mut vs = ViewSync::new();
//         vs.heard_view = 5;
//         assert!(vs.force_catch_up().is_some());
//         assert_eq!(vs.force_catch_up().unwrap(), 5);
//     }

//     #[test]
//     fn test_adaptive_timeout() {
//         let mut timeout = AdaptiveTimeout::new();
//         for rtt in vec![50, 60, 55, 200, 70, 65] {
//             timeout.record_rtt(rtt);
//         }
//         assert!(timeout.current_timeout() >= 200);  // P95 × 3
//     }
// }

//
// KG: SPAN_333_BFT_Transport_CodeStubs
// Ready to copy-paste and adapt for your implementation.
// Uncomment and fill in TODOs for complete integration.
//
