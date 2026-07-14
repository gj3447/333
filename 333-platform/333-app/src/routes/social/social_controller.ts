// KG: sprint5A-social-wasm-ui-2026-04-15
// KG: SOLID-DIP-fix-wasm-sessions-2026-04-16 — ISocialSession interface 경유
// Social Controller — UI↔WASM bridge for SocialProfile.

import { createSocialSession } from '../../lib/wasm-sessions';
import type { ISocialSession } from '../../lib/wasm-sessions';
//
// WASM binding status:
//  1. initSocial       → new wasm.SocialProfileWasm(peerId)        [WASM]
//  2. onPost           → wasm.profile.post_content(text)           [WASM]
//  3. onProfileUpdate  → wasm.profile.set_profile_field(k,v)       [WASM]
//  4. onFollow/Unfollow → wasm.profile.follow_peer / unfollow_peer [WASM]
//  5. poll feed        → wasm.profile.feed_json() + my_posts_json() [WASM]
//
// Mock mode: if WASM fails to load, local in-memory state is used.
// Real signaling integration is marked TODO_SIGNALING throughout.

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface SocialPost {
  id: string;
  author: number;
  content: string;
  timestamp: number;
  likes: number;
}

export interface SocialProfileFields {
  name: string;
  bio: string;
}

export interface SocialState {
  peerId: number;
  profile: SocialProfileFields;
  posts: SocialPost[];
  feed: SocialPost[];
  followers: number[];
  following: number[];
  wasmActive: boolean;
  log: string[];
}

// ---------------------------------------------------------------------------
// Mock state helpers
// ---------------------------------------------------------------------------

let _mockPostSeq = 0;

function mockPost(peerId: number, content: string): SocialPost {
  return {
    id: `post-${peerId}-${_mockPostSeq++}`,
    author: peerId,
    content,
    timestamp: Date.now(),
    likes: 0,
  };
}

// ---------------------------------------------------------------------------
// SocialController
// ---------------------------------------------------------------------------

export class SocialController {
  private state: SocialState;
  private onStateChange: (s: SocialState) => void;

  // WASM handle — ISocialSession interface 경유 (DIP 준수).
  // KG: sprint5A-social-wasm-ui-2026-04-15, SOLID-DIP-fix-wasm-sessions-2026-04-16
  private wasmProfile: ISocialSession | null = null;
  private _wasmAvailable = false;

  private _pollTimer: ReturnType<typeof setInterval> | null = null;

  constructor(onStateChange: (s: SocialState) => void) {
    this.onStateChange = onStateChange;
    this.state = {
      peerId: 1,
      profile: { name: '', bio: '' },
      posts: [],
      feed: [],
      followers: [],
      following: [],
      wasmActive: false,
      log: ['[social] controller ready'],
    };
  }

  // ---------------------------------------------------------------------------
  // Public API
  // ---------------------------------------------------------------------------

  /**
   * initSocial — load WASM module and create SocialProfileWasm(peerId).
   * Falls back to mock mode if WASM is unavailable.
   *
   * # KG: sprint5A-social-wasm-ui-2026-04-15
   */
  async initSocial(peerId: number): Promise<void> {
    this.state = { ...this.state, peerId };

    try {
      // createSocialSession factory (DIP: interface 경유, 직접 import 금지)
      // KG: SOLID-DIP-fix-wasm-sessions-2026-04-16
      this.wasmProfile = await createSocialSession(peerId);
      this._wasmAvailable = true;
      this._addLog(`[social] WASM init peerId=${peerId}`);
      this.state = { ...this.state, wasmActive: true };
    } catch (e) {
      this._wasmAvailable = false;
      this._addLog(`[social] WASM unavailable, mock mode (${String(e).slice(0, 60)})`);
    }

    this._startPoll();
    this._pushState();
  }

  /**
   * onPost — create a post with content text.
   * Returns the post_id string.
   *
   * # KG: sprint5A-social-wasm-ui-2026-04-15
   */
  onPost(content: string): string {
    if (!content.trim()) return '';

    if (this._wasmAvailable && this.wasmProfile) {
      try {
        const result = JSON.parse(this.wasmProfile.post_content(content)) as { post_id: string };
        this._addLog(`[post] ${result.post_id}`);
        this._syncFromWasm();
        return result.post_id;
      } catch (e) {
        this._addLog(`[post] WASM error: ${String(e).slice(0, 60)}`);
      }
    }

    // Mock fallback
    const post = mockPost(this.state.peerId, content);
    this.state = { ...this.state, posts: [post, ...this.state.posts] };
    this._addLog(`[post-mock] ${post.id}`);
    this._pushState();
    return post.id;
  }

  /**
   * onProfileUpdate — update a profile field (name, bio, …).
   *
   * # KG: sprint5A-social-wasm-ui-2026-04-15
   */
  onProfileUpdate(key: 'name' | 'bio', value: string): void {
    if (this._wasmAvailable && this.wasmProfile) {
      try {
        this.wasmProfile.set_profile_field(key, value);
      } catch { /* fallthrough */ }
    }
    // Update local mirror regardless
    this.state = { ...this.state, profile: { ...this.state.profile, [key]: value } };
    this._pushState();
  }

  /**
   * onFollow — follow another peer.
   *
   * # KG: sprint5A-social-wasm-ui-2026-04-15
   */
  onFollow(peerId: number): void {
    if (this._wasmAvailable && this.wasmProfile) {
      try { this.wasmProfile.follow_peer(peerId >>> 0); } catch { /* fallthrough */ }
      this._syncFromWasm();
    } else {
      if (!this.state.following.includes(peerId)) {
        this.state = { ...this.state, following: [...this.state.following, peerId] };
      }
      this._pushState();
    }
    this._addLog(`[follow] peer=${peerId}`);
  }

  /**
   * onUnfollow — unfollow a peer.
   *
   * # KG: sprint5A-social-wasm-ui-2026-04-15
   */
  onUnfollow(peerId: number): void {
    if (this._wasmAvailable && this.wasmProfile) {
      try { this.wasmProfile.unfollow_peer(peerId >>> 0); } catch { /* fallthrough */ }
      this._syncFromWasm();
    } else {
      this.state = { ...this.state, following: this.state.following.filter(id => id !== peerId) };
      this._pushState();
    }
    this._addLog(`[unfollow] peer=${peerId}`);
  }

  /**
   * isFollowing — check if this peer follows target.
   *
   * # KG: sprint5A-social-wasm-ui-2026-04-15
   */
  isFollowing(peerId: number): boolean {
    if (this._wasmAvailable && this.wasmProfile) {
      try { return this.wasmProfile.is_following(peerId >>> 0) as boolean; } catch { /* fallthrough */ }
    }
    return this.state.following.includes(peerId);
  }

  /**
   * ingestRemotePost — absorb a post from a remote peer (TODO_SIGNALING).
   *
   * # KG: sprint5A-social-wasm-ui-2026-04-15
   */
  ingestRemotePost(author: number, postJson: string): void {
    if (this._wasmAvailable && this.wasmProfile) {
      try {
        this.wasmProfile.ingest_remote_post(author >>> 0, postJson);
        this._syncFromWasm();
        return;
      } catch { /* fallthrough */ }
    }
    // Mock: add directly to feed
    try {
      const post = JSON.parse(postJson) as SocialPost;
      if (this.state.following.includes(author)) {
        const feed = [post, ...this.state.feed].sort((a, b) => b.timestamp - a.timestamp);
        this.state = { ...this.state, feed };
        this._pushState();
      }
    } catch { /* ignore malformed */ }
  }

  destroy(): void {
    this._stopPoll();
    if (this.wasmProfile) {
      try { this.wasmProfile.free?.(); } catch { /* ignore */ }
      this.wasmProfile = null;
    }
  }

  // ---------------------------------------------------------------------------
  // Private
  // ---------------------------------------------------------------------------

  /** Sync all state arrays from WASM profile snapshot. */
  private _syncFromWasm(): void {
    if (!this._wasmAvailable || !this.wasmProfile) return;
    try {
      const posts: SocialPost[] = JSON.parse(this.wasmProfile.my_posts_json() as string);
      const feed: SocialPost[] = JSON.parse(this.wasmProfile.feed_json() as string);
      const followers: number[] = JSON.parse(this.wasmProfile.followers_list() as string);
      const following: number[] = JSON.parse(this.wasmProfile.following_list() as string);

      // Keep profile fields from local mirror (WASM getter available but skipped for perf)
      this.state = { ...this.state, posts, feed, followers, following };
    } catch (e) {
      this._addLog(`[sync] WASM read error: ${String(e).slice(0, 60)}`);
    }
    this._pushState();
  }

  private _startPoll(): void {
    // Poll every 1 s for feed updates (skeleton — real signaling push is TODO_SIGNALING).
    // KG: sprint5A-social-wasm-ui-2026-04-15
    this._pollTimer = setInterval(() => {
      if (this._wasmAvailable) {
        this._syncFromWasm();
      }
    }, 1000);
  }

  private _stopPoll(): void {
    if (this._pollTimer !== null) {
      clearInterval(this._pollTimer);
      this._pollTimer = null;
    }
  }

  private _addLog(msg: string): void {
    this.state = { ...this.state, log: [...this.state.log.slice(-49), msg] };
  }

  private _pushState(): void {
    this.onStateChange({ ...this.state });
  }
}
