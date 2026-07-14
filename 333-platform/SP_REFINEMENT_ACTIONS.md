# SP Refinement Actions: 333_Platform (22 Contracts)

**Phase**: SP (SemanticPyramid) → ST (SemanticTwin) Gate
**Status**: POST-TALIBAN validation findings require resolution
**Owner**: Design Agent (before SCW transition)
**Timeline**: 2 hours clarification + revalidation

---

## Overview

Taliban validation of 22 ST Contracts found:
- **11 Existing** (HLC~Identity): APPROVED ✓ (code+tests verified)
- **11 Pending** (ORSet~Events): CONDITIONAL ⚠ (8 findings block SCW)

This document enumerates the 8 findings with specific Contract field updates required.

---

## Action Items

### CRITICAL (Resolve before re-validation)

#### F1: CONTRACT_333_ORSet — Element Uniqueness Strategy

**Finding**: Specification lacks unique_id mechanism. OR_Set requires each element to have a stable identity for concurrent add/remove semantics.

**Current Spec**:
```
ORSet<T> {
  add(item: T)
  remove(item: T)
  contains(item: T) -> bool
  merge_delta(delta: ORSetDelta<T>)
}
```

**Problem**: How are identity and tombstone tracking represented? Two strategies:

1. **Inline UUID** (4–16 bytes per element):
   - Pro: Self-contained, no external deps
   - Con: Bloats memory if T is already large
   - Code: `(uuid: u128, item: T)`

2. **External PeerId reference** (Identity.PeerId 32 bytes):
   - Pro: Reuses existing Identity contract
   - Con: Requires Identity layer available
   - Code: `HashMap<(peer_id: PeerId, clock: Lamport), T>`

**Required Contract Update**:
```rust
contract CONTRACT_333_ORSet {
  identity_strategy: enum {
    InlineUuid,          // 128-bit UUID per element
    PeerIdReference,     // Refer to CONTRACT_333_Identity
  },
  element_repr: struct {
    unique_id: String,   // Strategy-dependent: "uuid" or "peer_id+lamport"
    value: T,
    tombstone: bool,     // true after remove()
    timestamp: Lamport,  // Causality tracking
  },
  gc_enabled: bool,      // Compact tombstones? Add gc() method?
}
```

**Action**: Choose strategy → update Contract fields → code reflects choice.

---

#### F2: CONTRACT_333_RGA — Position Encoding Strategy

**Finding**: RGA (Replicated Growable Array) requires position encoding for order preservation in concurrent inserts. Spec omits this critical detail.

**Problem**: How are positions assigned? Competing designs:

1. **Fractional Indexing** (e.g., Yjs, Notion):
   - Insert between positions: pos[i] and pos[i+1]
   - Assign fractional: (pos[i] + pos[i+1]) / 2
   - Pro: Simple, O(1) insert
   - Con: Risk of floating-point precision loss over time

2. **Interval Trees** (e.g., CRDT literature):
   - Store (start, end) intervals for each element
   - Insert: subdivide interval
   - Pro: Mathematically clean
   - Con: More memory, more complex merge

3. **Timestamp-based ordering** (timestamp, peer_id):
   - Each insertion gets (Lamport/HLC, peer_id)
   - Order by timestamp, then peer_id tiebreaker
   - Pro: Aligns with HLC/Lamport contracts
   - Con: Lost position info if timestamps are close

**Required Contract Update**:
```rust
contract CONTRACT_333_RGA {
  position_strategy: enum {
    FractionalIndexing,  // (a+b)/2 between elements
    IntervalTrees,       // (start, end) intervals
    TimestampOrdering,   // (Lamport/HLC, PeerId) sort
  },
  position_repr: struct {
    strategy_specific_field: T,  // e.g., fraction f64, interval [u64,u64], or (Lamport, PeerId)
  },
  delete_semantics: enum {
    Tombstone,           // Keep entry, mark deleted
    LogicalCompact,      // Lazy compaction, rebuild on access
  },
}
```

**Action**: Research Prause et al. RGA paper → choose strategy → update Contract.

---

#### F3: CONTRACT_333_DataChannel — STUN/TURN Server Configuration

**Finding**: WebRTC DataChannel requires ICE candidates for NAT traversal. Spec omits server endpoints, creating deployment ambiguity.

**Problem**: ICE candidates come from STUN/TURN servers. Where are they hosted?

1. **AWS STUN/TURN**:
   - Pro: Managed, high availability
   - Con: Cost ~$0.30 per 1M messages, vendor lock
   - Config: `turn:turn.aws.example.com:3478`

2. **Custom on-premises TURN** (coturn, etc.):
   - Pro: Full control, zero marginal cost
   - Con: Ops burden, HA/failover needed
   - Config: Deploy on same infrastructure as Signaling server

3. **Hybrid** (STUN → AWS, TURN → custom):
   - Pro: Cost-optimal (STUN is cheap), control fallback
   - Con: Complex failover logic

**Required Contract Update**:
```rust
contract CONTRACT_333_DataChannel {
  stun_servers: Vec<String>,  // e.g., ["stun.l.google.com:19302", ...]
  turn_servers: Vec<TurnServer>,

  struct TurnServer {
    url: String,       // e.g., "turn:turn.example.com:3478"
    username: Option<String>,
    credential: Option<String>,  // ephemeral or static?
    credential_type: enum { StaticPassword, Oauth, Ephemeral },
  },

  connection_timeout_ms: u32,  // ICE gathering + connection max time
  ice_transport_policy: enum {
    All,       // Both STUN + TURN
    Relay,     // TURN only (privacy mode)
  },
}
```

**Action**: Choose STUN/TURN strategy → define config struct → add to Contract.

---

#### F4: CONTRACT_333_MeshRoom — Peer Failure Detection + Lifecycle

**Finding**: MeshRoom lacks peer health monitoring and state transitions. In a mesh topology with browser tabs that can close abruptly, failure detection is critical.

**Problem**: How do peers know if others are alive? Design choices:

1. **Heartbeat-based**:
   - Each peer sends periodic ping every N ms
   - Peer marked dead after K missed pings
   - Pro: Simple, standard
   - Con: O(n) bandwidth per peer

2. **Timeout-based** (implicit):
   - No activity in T ms → remove peer
   - Pro: Bandwidth-efficient
   - Con: Asymmetric knowledge (peer might think you're alive while you think it's dead)

3. **Gossip-based** (hybrid):
   - Peers gossip about whom they've seen
   - Quorum consensus on liveness
   - Pro: Byzantine-tolerant
   - Con: Complex, overkill for mesh?

**Also missing**: Room lifecycle states. Currently no distinction between:
- OPEN: accepting new peers
- CLOSING: no new peers, but existing can finish
- CLOSED: room fully torn down

**Required Contract Update**:
```rust
contract CONTRACT_333_MeshRoom {
  room_state: enum {
    Open,      // accept new joins
    Closing,   // no new joins, graceful shutdown
    Closed,    // fully terminated
  },

  peer_health: {
    heartbeat_interval_ms: u32,  // ping frequency
    heartbeat_timeout_ms: u32,   // missed pings before removal
    max_peers: u32,              // mesh scalability limit (n peers = n(n-1)/2 connections)
  },

  lifecycle: {
    auto_close_if_empty_ms: u32,  // close room if last peer left
    graceful_shutdown_timeout_ms: u32,
  },
}
```

**Action**: Define heartbeat + state machine → add to Contract → cite mesh topology paper.

---

### HIGH (Clarify before re-validation)

#### F5: CONTRACT_333_DHT — Node ID Assignment & Format

**Finding**: DHT node IDs must match Identity contract, but no unification specified.

**Current ambiguity**: Are DHT node IDs:
- Numeric (u64)? sha256(public_key)[0:8]?
- 32-byte (full PeerId)? Just the Lamport counter?

**Impact**: KademliaDHT distance metric (XOR) requires fixed-size node IDs. Spec must clarify.

**Required Contract Update**:
```rust
contract CONTRACT_333_DHT {
  node_id_format: enum {
    PeerId32,          // full 32-byte from Identity.PeerId
    Sha256Prefix8,     // sha256(pubkey)[0:8] as u64
    LamportCounter,    // just the logical clock?
  },

  // Must match chosen format
  distance_metric: "XOR(node_id)",

  // Ensure cross-contract consistency
  depends_on: ["CONTRACT_333_Identity"],
}
```

**Action**: Align with Identity contract → specify node_id_format → code must use same ID scheme.

---

#### F6: CONTRACT_333_Signaling — Authentication & Rate Limiting

**Finding**: Cloudflare Workers relay is currently unprotected. DDoS risk: attacker floods SDP offers → Workers quota exhausted.

**Missing**:
1. **Authentication**: How does client prove it owns a room?
2. **Rate Limiting**: Max requests per minute per IP?
3. **Stale SDP Cleanup**: Who deletes old offers? When?

**Required Contract Update**:
```rust
contract CONTRACT_333_Signaling {
  auth: {
    method: enum {
      HmacSha256,      // Client signs requests with room_id + secret
      ApiKey,          // Stateless API key in header
      Hmac = HmacSha256,
    },
    secret_derivation: "sha256(room_id + master_secret)?",
  },

  rate_limiting: {
    requests_per_minute: u32,  // e.g., 100
    burst_allowed: u32,        // e.g., 10
  },

  stale_sdp_policy: {
    ttl_seconds: u32,          // e.g., 3600 (1 hour)
    cleanup_trigger: "periodic | on_request",
  },

  error_responses: {
    "400 BadRequest": "Invalid signature or format",
    "429 TooManyRequests": "Rate limit exceeded",
    "410 Gone": "SDP offer expired",
  },
}
```

**Action**: Add auth + rate limiting + cleanup → spec error codes → workers code must validate.

---

#### F7: CONTRACT_333_IndexedDB — Schema Versioning & Migration

**Finding**: CRDT contracts (ORSet, RGA, LwwMap) will evolve. IndexedDB needs a migration strategy for apps to update their stored schemas.

**Problem**: What if v1 of my app stored `{key: string, value: T}` but v2 stores `{key: string, value: T, timestamp: Lamport}`? How does the app transition?

**Required Contract Update**:
```rust
contract CONTRACT_333_IndexedDB {
  schema_versioning: {
    current_version: u32,
    migrations: Vec<Migration>,  // v1→v2, v2→v3, etc.
  },

  struct Migration {
    from_version: u32,
    to_version: u32,
    handler: fn(old_data: T) -> T,  // transform function
  },

  backup_restore: {
    supports_backup: bool,
    backup_format: "JSON | Binary",
    restore_handler: Option<fn(backup: Bytes) -> Result<T>>,
  },
}
```

**Action**: Define schema migration API → add to Contract → AppSDK must call migrations on startup.

---

#### F8: CONTRACT_333_WireProtocol — Version Field

**Finding**: Binary protocol has 4-byte header: `type:u8 | flags:u8 | len:u16`. No room for version. If message types grow, old clients can't parse new messages.

**Problem**: Current design is rigid. Need forward/backward compatibility strategy.

**Option 1: Reserve version bits**:
```
type: u8 → type:u4 | version:u4
```
But this limits message types to 16 (too few for future).

**Option 2: Expand header to 5 bytes**:
```
version: u8 | type: u8 | flags: u8 | len: u16
```
Pro: Explicit version field. Con: breaks existing wire format.

**Option 3: Version in flags**:
```
flags: u8 → reserved_version:u4 | actual_flags:u4
```
Clever but confusing.

**Required Contract Update**:
```rust
contract CONTRACT_333_WireProtocol {
  header_format: enum {
    Current4Byte,      // version:u8 | type:u8 | flags:u8 | len:u16
    Legacy,            // (if v1 exists, doc difference)
  },

  version_strategy: {
    current_version: u8,
    max_version: u8,   // e.g., 15 if 4-bit version
    compatibility: enum {
      Strict,          // reject mismatched versions
      Lenient,         // attempt backward-compat parsing
    },
  },

  message_types: {
    max_types: u8,  // limits based on version field size
  },
}
```

**Action**: Choose version strategy → update header spec → document wire format version history.

---

## Refinement Workflow

### Step 1: Update Contract Specs (not code yet)
For each of the 8 findings (F1–F8), update the Contract YAML/JSON with new fields:

```yaml
contract CONTRACT_333_ORSet:
  name: ORSet
  purpose: Concurrent set with add/remove/merge semantics
  responsibility: CRDT set state management
  fields:
    - identity_strategy: enum[InlineUuid|PeerIdReference]  # NEW
    - element_repr: struct{unique_id, value, tombstone, timestamp}  # NEW
    - gc_enabled: bool  # NEW
  dependencies:
    - CONTRACT_333_Identity (if strategy=PeerIdReference)
    - CONTRACT_333_Lamport
  tests:
    - concurrent_add_remove_ordering
    - merge_delta_idempotent
    - gc_tombstone_cleanup  # NEW
```

### Step 2: Revalidate with Taliban
```
FOR each updated Contract in (12..22):
  Taliban.validate(contract, 9_lens)
  IF any lens fails:
    RETURN to Step 1
  ELSE:
    Mark contract as VALIDATED
```

### Step 3: Gate Approval
```
IF all(contracts[1..22].status == VALIDATED):
  → VR_333_Platform_ST_20260405.verdict ← APPROVED
  → UNLOCK SCW phase
ELSE:
  → Return to SP, add new Spans for unresolved findings
```

---

## KG Action Items

```cypher
-- Record each refinement action
MATCH (c:AptContract {name: 'CONTRACT_333_ORSet'})
MERGE (a:RefinementAction {name: 'RA_F1_ORSet_Identity'})
SET a.finding = 'F1',
    a.description = 'Define identity_strategy in CONTRACT_333_ORSet',
    a.status = 'PENDING',
    a.owner = 'Design',
    a.due_date = date('2026-04-05') + duration({hours: 2})
MERGE (c)-[:REQUIRES_ACTION]->(a)

-- After refinement, update validation result
MATCH (vr:ValidationResult {name: 'VR_333_Platform_ST_20260405'})
SET vr.verdict = 'APPROVED',
    vr.refinement_completed_at = datetime()
```

---

## Summary

| Finding | Type | Contract | Action | Owner | ETA |
|---------|------|----------|--------|-------|-----|
| F1 | Critical | ORSet | Choose identity strategy (UUID vs PeerId) | Design | 15 min |
| F2 | Critical | RGA | Research + choose position encoding | Design | 30 min |
| F3 | Critical | DataChannel | Define STUN/TURN server config | Infra/Design | 20 min |
| F4 | Critical | MeshRoom | Add heartbeat + lifecycle states | Design | 30 min |
| F5 | High | DHT | Unify node ID format with Identity | Design | 15 min |
| F6 | High | Signaling | Add auth + rate limiting | Design/Security | 30 min |
| F7 | High | IndexedDB | Define schema migration API | Design | 20 min |
| F8 | High | WireProtocol | Choose version strategy | Design | 15 min |
| **Total** | | | | | **~3 hours** |

---

**Next Phase**: SP Refinement → Taliban Re-validation → ST→SCW Gate Approval

**References**:
- Full validation: `TALIBAN_ST_VALIDATION.md`
- Brief matrix: `TALIBAN_ST_BRIEF.md`
- KG: `VR_333_Platform_ST_20260405`
