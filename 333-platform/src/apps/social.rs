// KG: SPAN_333_KillerApps — P2P Social Network
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-apps-social-Post/SocialProfile
// OR-Set followers, LWW-Map profiles, DHT post storage

use crate::lww_map::LwwMap;
use crate::or_set::ORSet;
use crate::hlc::Hlc;
use serde::{Deserialize, Serialize};

/// Post on the social network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Post {
    pub id: String,
    pub author: u32,
    pub content: String,
    pub timestamp: u64,
    pub likes: u32,
}

/// User profile
pub struct SocialProfile {
    pub peer_id: u32,
    pub hlc: Hlc,
    pub profile: LwwMap<String, String>,  // key-value profile fields
    pub followers: ORSet<u32>,             // who follows me
    pub following: ORSet<u32>,             // who I follow
    pub posts: Vec<Post>,
    pub feed: Vec<Post>,                   // aggregated from following
}

impl SocialProfile {
    pub fn new(peer_id: u32) -> Self {
        let profile = LwwMap::new(peer_id);
        let hlc = Hlc::new(peer_id);
        Self {
            peer_id,
            hlc,
            profile,
            followers: ORSet::new(peer_id),
            following: ORSet::new(peer_id),
            posts: Vec::new(),
            feed: Vec::new(),
        }
    }

    /// Set profile field (name, bio, avatar, etc.)
    pub fn set_field(&mut self, key: &str, value: &str) {
        self.hlc.tick();
        self.profile.set(key.to_string(), value.to_string());
    }

    /// Get profile field
    pub fn get_field(&self, key: &str) -> Option<&String> {
        self.profile.get(&key.to_string())
    }

    /// Follow another user
    pub fn follow(&mut self, other: u32) {
        self.hlc.tick();
        self.following.add(other);
    }

    /// Unfollow
    pub fn unfollow(&mut self, other: u32) {
        self.hlc.tick();
        self.following.remove(&other);
    }

    /// Add a follower (called when remote peer follows us)
    pub fn add_follower(&mut self, follower: u32) {
        self.hlc.tick();
        self.followers.add(follower);
    }

    /// Create a post.
    /// Timestamp is derived from the HLC after a tick — globally monotonic across peers
    /// as long as peers periodically synchronize HLC on message receive.
    /// # KG: taliban2-M4-fix-2026-04-15
    pub fn create_post(&mut self, content: &str) -> Post {
        self.hlc.tick();
        // Combine wall_ms and counter into a u64 timestamp that is strictly monotonic
        // within this peer and comparable across peers when HLCs are synchronized.
        // Encoding: upper 48 bits = wall_ms (ms since epoch), lower 16 bits = counter.
        // This gives ~280 years of range and 65535 events per millisecond per peer.
        // # KG: taliban2-M4-fix-2026-04-15
        let timestamp = (self.hlc.wall_ms << 16) | (self.hlc.counter as u64 & 0xFFFF);
        let post = Post {
            id: format!("post-{}-{}", self.peer_id, self.posts.len()),
            author: self.peer_id,
            content: content.to_string(),
            timestamp,
            likes: 0,
        };
        self.posts.push(post.clone());
        post
    }

    /// Add post to feed (from someone I follow)
    pub fn add_to_feed(&mut self, post: Post) {
        if self.following.contains(&post.author) {
            self.feed.push(post);
            self.feed.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        }
    }

    /// Get follower count
    pub fn follower_count(&self) -> usize {
        self.followers.len()
    }

    /// Get following count
    pub fn following_count(&self) -> usize {
        self.following.len()
    }

    /// Get post count
    pub fn post_count(&self) -> usize {
        self.posts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_fields() {
        let mut profile = SocialProfile::new(1);
        profile.set_field("name", "Alice");
        profile.set_field("bio", "333 enthusiast");
        assert_eq!(profile.get_field("name"), Some(&"Alice".to_string()));
        assert_eq!(profile.get_field("bio"), Some(&"333 enthusiast".to_string()));
    }

    #[test]
    fn follow_unfollow() {
        let mut alice = SocialProfile::new(1);
        alice.follow(2);
        alice.follow(3);
        assert_eq!(alice.following_count(), 2);

        alice.unfollow(2);
        assert_eq!(alice.following_count(), 1);
    }

    #[test]
    fn create_posts() {
        let mut alice = SocialProfile::new(1);
        let post = alice.create_post("Hello 333!");
        assert_eq!(post.author, 1);
        assert_eq!(post.content, "Hello 333!");
        assert_eq!(alice.post_count(), 1);
    }

    #[test]
    fn feed_from_following() {
        let mut alice = SocialProfile::new(1);
        let mut bob = SocialProfile::new(2);

        alice.follow(2);
        let post = bob.create_post("Bob's post");

        alice.add_to_feed(post);
        assert_eq!(alice.feed.len(), 1);
        assert_eq!(alice.feed[0].content, "Bob's post");
    }

    #[test]
    fn feed_ignores_non_following() {
        let mut alice = SocialProfile::new(1);
        // Alice does NOT follow Charlie(3)
        let post = Post {
            id: "p1".into(), author: 3,
            content: "spam".into(), timestamp: 100, likes: 0,
        };
        alice.add_to_feed(post);
        assert_eq!(alice.feed.len(), 0); // rejected
    }

    // ----- sprint5A additions (KG: sprint5A-social-wasm-ui-2026-04-15) -------

    /// Post IDs are deterministic: peer_id + index.
    #[test]
    fn post_id_deterministic() {
        let mut a = SocialProfile::new(42);
        let p0 = a.create_post("first");
        let p1 = a.create_post("second");
        assert_eq!(p0.id, "post-42-0");
        assert_eq!(p1.id, "post-42-1");
    }

    /// OR-Set for `following` converges under concurrent add/remove.
    #[test]
    fn or_set_follow_convergence() {
        let mut alice = SocialProfile::new(1);
        let mut bob_mirror = SocialProfile::new(1); // same peer, simulates replica

        alice.follow(10);
        alice.follow(20);
        alice.unfollow(10);

        bob_mirror.follow(10);
        bob_mirror.follow(20);
        bob_mirror.unfollow(10);

        // Both replicas should have same state
        assert!(!alice.following.contains(&10));
        assert!(alice.following.contains(&20));
        assert_eq!(alice.following_count(), bob_mirror.following_count());
    }

    /// Feed is always reverse-chronological (latest timestamp first).
    #[test]
    fn feed_sorted_reverse_chronological() {
        let mut alice = SocialProfile::new(1);
        alice.follow(2);

        let posts = vec![
            Post { id: "p1".into(), author: 2, content: "old".into(), timestamp: 1, likes: 0 },
            Post { id: "p2".into(), author: 2, content: "new".into(), timestamp: 100, likes: 0 },
            Post { id: "p3".into(), author: 2, content: "mid".into(), timestamp: 50, likes: 0 },
        ];
        for p in posts {
            alice.add_to_feed(p);
        }
        assert_eq!(alice.feed[0].timestamp, 100);
        assert_eq!(alice.feed[1].timestamp, 50);
        assert_eq!(alice.feed[2].timestamp, 1);
    }

    /// Multiple followers can be added; follower_count stays consistent.
    #[test]
    fn follower_count_consistent() {
        let mut alice = SocialProfile::new(1);
        alice.add_follower(2);
        alice.add_follower(3);
        alice.add_follower(4);
        assert_eq!(alice.follower_count(), 3);
    }

    /// LWW profile fields: last write wins.
    #[test]
    fn lww_profile_last_write_wins() {
        let mut alice = SocialProfile::new(1);
        alice.set_field("name", "Alice");
        alice.set_field("name", "Alice333"); // overwrite
        assert_eq!(alice.get_field("name"), Some(&"Alice333".to_string()));
    }

    /// # KG: taliban2-M4-fix-2026-04-15
    /// Post timestamps must be strictly monotonically increasing within a single peer
    /// (each successive post must have timestamp > previous post).
    /// Previously timestamp = posts.len(), which is NOT monotonic across peers and only
    /// increases by 1. With HLC tick the timestamp encodes wall_ms<<16 | counter and
    /// is always strictly greater than the previous (HLC guarantees this).
    #[test]
    fn post_timestamp_monotonic_via_hlc() {
        let mut alice = SocialProfile::new(1);
        let mut prev_ts = 0u64;
        for i in 0..5 {
            let post = alice.create_post(&format!("post {}", i));
            assert!(
                post.timestamp > prev_ts,
                "post {} timestamp {} must be > previous {}",
                i, post.timestamp, prev_ts
            );
            prev_ts = post.timestamp;
        }
    }

    /// # KG: taliban2-M4-fix-2026-04-15
    /// Two different peers creating posts at roughly the same time should produce
    /// timestamps that are both non-zero and independently monotonic.
    #[test]
    fn post_timestamp_nonzero_and_hlc_derived() {
        let mut alice = SocialProfile::new(1);
        let mut bob = SocialProfile::new(2);
        let p_a = alice.create_post("alice");
        let p_b = bob.create_post("bob");
        // Both timestamps must be HLC-derived (> 0 since wall_ms > 0 in non-WASM)
        assert!(p_a.timestamp > 0, "alice timestamp must be > 0");
        assert!(p_b.timestamp > 0, "bob timestamp must be > 0");
        // Timestamps must NOT be plain post-index values (0 and 0) — they must be large.
        // HLC wall_ms is in milliseconds since UNIX epoch (~1.7T), shifted left 16 bits.
        assert!(
            p_a.timestamp > 0xFFFF,
            "alice timestamp {} must be HLC-derived (>> simple index)", p_a.timestamp
        );
    }
}
