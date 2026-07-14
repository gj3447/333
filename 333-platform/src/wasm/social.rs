// KG: sprint5A-social-wasm-ui-2026-04-15
//! WASM bindings for SocialProfile — exposes P2P social graph to JavaScript.
//!
//! Wraps `SocialProfile` (OR-Set followers, LWW profile, posts, feed) behind a
//! `#[wasm_bindgen]` facade. All JSON serialisation stays in this layer; inner
//! crate types stay JS-free.
//!
//! # API surface
//! - `SocialProfileWasm::new(peer_id)` — create profile for local peer
//! - `post_content(text)` → post_id JSON string
//! - `set_profile_field(key, value)` / `get_profile_field(key)` → Option<String>
//! - `follow_peer(peer_id)` / `unfollow_peer(peer_id)` / `is_following(peer_id)`
//! - `followers_list()` / `following_list()` → JSON array of u32
//! - `my_posts_json()` / `feed_json()` → JSON arrays of Post
//! - `ingest_remote_post(author, post_json)` → absorb a remote post into feed
//! - `merge_remote_followers(or_set_state_json)` → CRDT OR-Set merge (stub)
//! - `serialize_state()` → full snapshot JSON

use wasm_bindgen::prelude::*;
use crate::apps::social::{Post, SocialProfile};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Wire types (private — JSON boundary only)
// ---------------------------------------------------------------------------

/// Minimal incoming post shape for `ingest_remote_post`.
#[derive(Deserialize)]
struct RemotePost {
    id: String,
    #[allow(dead_code)]
    author: u32,
    content: String,
    timestamp: u64,
    #[serde(default)]
    likes: u32,
}

// ---------------------------------------------------------------------------
// SocialProfileWasm
// ---------------------------------------------------------------------------

/// WASM-exposed wrapper around `SocialProfile`.
///
/// Created via JS: `new wasm.SocialProfileWasm(peerId)`
///
/// KG: sprint5A-social-wasm-ui-2026-04-15
#[wasm_bindgen]
pub struct SocialProfileWasm {
    inner: SocialProfile,
}

#[wasm_bindgen]
impl SocialProfileWasm {
    /// Create a new social profile for `peer_id`.
    ///
    /// KG: sprint5A-social-wasm-ui-2026-04-15
    #[wasm_bindgen(constructor)]
    pub fn new(peer_id: u32) -> Self {
        console_error_panic_hook::set_once();
        Self { inner: SocialProfile::new(peer_id) }
    }

    // -------------------------------------------------------------------------
    // Post
    // -------------------------------------------------------------------------

    /// Create a post with `text` and return the post_id as a JSON string.
    ///
    /// KG: sprint5A-social-wasm-ui-2026-04-15
    pub fn post_content(&mut self, text: &str) -> String {
        let post = self.inner.create_post(text);
        serde_json::json!({ "post_id": post.id }).to_string()
    }

    // -------------------------------------------------------------------------
    // Profile fields
    // -------------------------------------------------------------------------

    /// Set a profile field (name, bio, avatar …).
    ///
    /// KG: sprint5A-social-wasm-ui-2026-04-15
    pub fn set_profile_field(&mut self, key: &str, value: &str) {
        self.inner.set_field(key, value);
    }

    /// Get a profile field — returns `None` if the key doesn't exist.
    ///
    /// KG: sprint5A-social-wasm-ui-2026-04-15
    pub fn get_profile_field(&self, key: &str) -> Option<String> {
        self.inner.get_field(key).cloned()
    }

    // -------------------------------------------------------------------------
    // Follow graph
    // -------------------------------------------------------------------------

    /// Follow another peer.
    ///
    /// KG: sprint5A-social-wasm-ui-2026-04-15
    pub fn follow_peer(&mut self, peer_id: u32) {
        self.inner.follow(peer_id);
    }

    /// Unfollow a peer.
    ///
    /// KG: sprint5A-social-wasm-ui-2026-04-15
    pub fn unfollow_peer(&mut self, peer_id: u32) {
        self.inner.unfollow(peer_id);
    }

    /// Returns true if this peer is following `peer_id`.
    ///
    /// KG: sprint5A-social-wasm-ui-2026-04-15
    pub fn is_following(&self, peer_id: u32) -> bool {
        self.inner.following.contains(&peer_id)
    }

    /// JSON array of peer_ids following *this* user (people who follow me).
    ///
    /// KG: sprint5A-social-wasm-ui-2026-04-15
    pub fn followers_list(&self) -> String {
        let ids: Vec<u32> = self.inner.followers.iter().copied().collect();
        serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string())
    }

    /// JSON array of peer_ids *this* user follows.
    ///
    /// KG: sprint5A-social-wasm-ui-2026-04-15
    pub fn following_list(&self) -> String {
        let ids: Vec<u32> = self.inner.following.iter().copied().collect();
        serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string())
    }

    // -------------------------------------------------------------------------
    // Feed / posts
    // -------------------------------------------------------------------------

    /// JSON array of this peer's own posts.
    ///
    /// KG: sprint5A-social-wasm-ui-2026-04-15
    pub fn my_posts_json(&self) -> String {
        serde_json::to_string(&self.inner.posts).unwrap_or_else(|_| "[]".to_string())
    }

    /// Aggregated feed JSON (posts from peers I follow, reverse-chronological).
    ///
    /// KG: sprint5A-social-wasm-ui-2026-04-15
    pub fn feed_json(&self) -> String {
        serde_json::to_string(&self.inner.feed).unwrap_or_else(|_| "[]".to_string())
    }

    // -------------------------------------------------------------------------
    // Remote ingestion
    // -------------------------------------------------------------------------

    /// Absorb a post authored by `author` into the local feed.
    ///
    /// `post_json` must be a JSON object matching the `Post` schema:
    /// `{ id, author, content, timestamp, likes }`.
    ///
    /// Returns an error string (for JS) if parsing fails.
    ///
    /// KG: sprint5A-social-wasm-ui-2026-04-15
    pub fn ingest_remote_post(&mut self, author: u32, post_json: &str) -> Result<(), JsValue> {
        let remote: RemotePost = serde_json::from_str(post_json)
            .map_err(|e| JsValue::from_str(&format!("parse error: {e}")))?;

        let post = Post {
            id: remote.id,
            author,
            content: remote.content,
            timestamp: remote.timestamp,
            likes: remote.likes,
        };

        self.inner.add_to_feed(post);
        Ok(())
    }

    /// Merge a remote followers OR-Set snapshot into the local followers set.
    ///
    /// Accepts a JSON array of peer_id u32 values (compact snapshot format).
    /// Full CRDT delta merge requires tag exchange — this is the peer-snapshot path.
    ///
    /// KG: sprint5A-social-wasm-ui-2026-04-15
    pub fn merge_remote_followers(&mut self, or_set_state_json: &str) -> Result<(), JsValue> {
        let peer_ids: Vec<u32> = serde_json::from_str(or_set_state_json)
            .map_err(|e| JsValue::from_str(&format!("parse error: {e}")))?;

        for pid in peer_ids {
            if !self.inner.followers.contains(&pid) {
                self.inner.add_follower(pid);
            }
        }
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Snapshot
    // -------------------------------------------------------------------------

    /// Serialise the full profile state to JSON.
    ///
    /// Shape:
    /// ```json
    /// {
    ///   "peer_id": 1,
    ///   "profile_fields": [["name","Alice"],...],
    ///   "followers": [2,3],
    ///   "following": [4],
    ///   "posts": [...],
    ///   "feed": [...]
    /// }
    /// ```
    ///
    /// KG: sprint5A-social-wasm-ui-2026-04-15
    pub fn serialize_state(&self) -> String {
        let snap = serde_json::json!({
            "peer_id": self.inner.peer_id,
            "followers": self.inner.followers.iter().copied().collect::<Vec<u32>>(),
            "following": self.inner.following.iter().copied().collect::<Vec<u32>>(),
            "posts": &self.inner.posts,
            "feed": &self.inner.feed,
        });
        snap.to_string()
    }

    // -------------------------------------------------------------------------
    // Utility
    // -------------------------------------------------------------------------

    /// Return this peer's id.
    pub fn peer_id(&self) -> u32 {
        self.inner.peer_id
    }

    /// Return follower count.
    pub fn follower_count(&self) -> u32 {
        self.inner.follower_count() as u32
    }

    /// Return following count.
    pub fn following_count(&self) -> u32 {
        self.inner.following_count() as u32
    }

    /// Return post count.
    pub fn post_count(&self) -> u32 {
        self.inner.post_count() as u32
    }
}
