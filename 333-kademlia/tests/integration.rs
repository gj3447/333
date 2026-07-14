// KG: SPAN_333_L2_Kademlia integration
// End-to-end: 100 peers joining, find_closest returns correct top-k.

use identity333::Keypair;
use kademlia333::{Distance, RoutingTable};

#[test]
fn simulate_100_peer_join() {
    let me = Keypair::from_seed([0u8; 32]).node_id();
    let mut rt = RoutingTable::new(me);

    let peers: Vec<_> = (1u8..=100)
        .map(|i| Keypair::from_seed([i; 32]).node_id())
        .collect();

    for p in &peers {
        rt.observe(*p);
    }

    // Not all 100 necessarily fit: bucket capacity may evict. But expect most to be present.
    let count = rt.len();
    assert!(count >= 20, "expected at least 20 peers across buckets, got {count}");

    // find_closest returns top-k by XOR distance.
    let target = peers[50];
    let closest = rt.find_closest(&target, 5);
    assert!(!closest.is_empty());

    // First result is closest of everything currently in the table.
    let all_peers: Vec<_> = rt.find_closest(&target, rt.len());
    let mut sorted = all_peers.clone();
    sorted.sort_by_key(|n| Distance::between(n, &target));
    assert_eq!(all_peers, sorted, "find_closest ordering must be by XOR distance");
}

#[test]
fn target_itself_is_top1() {
    let me = Keypair::from_seed([1u8; 32]).node_id();
    let mut rt = RoutingTable::new(me);

    let target = Keypair::from_seed([42u8; 32]).node_id();
    rt.observe(target);
    // add some noise
    for i in 100u8..120 {
        rt.observe(Keypair::from_seed([i; 32]).node_id());
    }

    let closest = rt.find_closest(&target, 3);
    assert_eq!(closest[0], target, "target is its own closest peer");
}
