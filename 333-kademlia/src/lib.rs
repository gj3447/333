// KG: SPAN_333_L2_Kademlia, plan-333-three-way-hybrid-2026-04-17
// Minimal Kademlia DHT over identity333::NodeId (32-byte Ed25519 pubkey = 256-bit key space).
//
// Eviction policy note: the `KBucket` evicts the least-recently-seen node
// unconditionally when full. This is a deliberate simplification of the
// Kademlia paper (Maymounkov & Mazières §4.1), which recommends *pinging*
// the oldest node and keeping it if it responds. Implementing the ping flow
// requires a transport layer (L1) and is scheduled for a later milestone.

#![forbid(unsafe_code)]

pub mod distance;
pub mod kbucket;
pub mod routing_table;

pub use distance::Distance;
pub use kbucket::{KBucket, K_DEFAULT};
pub use routing_table::RoutingTable;
