# Arc<Mutex> → Rc<RefCell> Quick Reference
**For 333-Platform WebRTC WASM Migration**

## Find & Replace Cheatsheet

### Imports
```bash
# Find all Arc/Mutex imports
grep -r "use std::sync::{Arc, Mutex}" src/

# Replace with
use std::rc::Rc;
use std::cell::RefCell;
```

### Type Declarations

| Find | Replace |
|------|---------|
| `Arc<Mutex<T>>` | `Rc<RefCell<T>>` |
| `Arc::new(Mutex::new(x))` | `Rc::new(RefCell::new(x))` |
| `Arc::clone(&x)` | `Rc::clone(&x)` |

### Method Calls

| Context | Find | Replace | Why |
|---------|------|---------|-----|
| Immutable borrow | `.lock().unwrap()` | `.borrow()` | WASM doesn't need mutex |
| Mutable borrow | `.lock().unwrap()` | `.borrow_mut()` | Single-threaded |
| Safe fallible | `.lock()` | `.try_borrow()` | Runtime check only |
| Safe fallible mut | N/A | `.try_borrow_mut()` | Prevents panics |

---

## Decision Tree: Which Pattern to Use?

```
Do you need to prevent panics at runtime?
├─ YES → Use try_borrow() / try_borrow_mut()
│        Return Result<T, BorrowError>
│
└─ NO  → Use borrow() / borrow_mut()
         More ergonomic, panics on conflict

Are you inside a closure (event handler)?
├─ YES → Use borrow_mut() directly
│        Closure manages lifetime
│
└─ NO  → Scope the borrow with {}
         Ensure borrow is released before next operation

Is this in a hot path (called 1000+ times/sec)?
├─ YES → Consider try_borrow_mut() to avoid panics
│        Log skipped operations
│
└─ NO  → borrow_mut() is fine
         Simpler code, negligible overhead
```

---

## Borrow Error Panic Checklist

**Before migration, add logging to catch BorrowErrors:**

```rust
// Add to your wasm crate
#[cfg(target_arch = "wasm32")]
pub fn setup_panic_logging() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

// In main / initialization
setup_panic_logging();
```

**In browser console (DevTools → Console), watch for:**
```
panicked at 'already borrowed: BorrowMutError'
panicked at 'already borrowed mutably: BorrowError'
```

If you see these, **the closure is trying to borrow while another closure holds the borrow**.

---

## Files to Modify

### Priority 1 (Simple, Low Risk)
- [ ] `src/p2p/channel.rs` line 66
  - Type alias only: `type SharedQueue = ...`
  - 2-3 `.lock().unwrap()` → `.borrow_mut()`
  - **Estimated impact:** 3 minutes

### Priority 2 (Core Change, Higher Risk)
- [ ] `src/p2p/webrtc.rs`
  - Struct fields (lines 24-25)
  - Constructor (lines 60-61)
  - Multiple closures (lines 113-136)
  - 15+ method conversions
  - **Estimated impact:** 20-30 minutes + testing

### Priority 3 (If Used)
- [ ] `src/bft/transport.rs`
  - Check for `Arc<Mutex>` usage
  - Similar pattern to webrtc.rs
  - **Estimated impact:** 15 minutes

---

## Line-by-Line Changes for webrtc.rs

```rust
Line 15:
  - use std::sync::{Arc, Mutex};
  + use std::rc::Rc;
  + use std::cell::RefCell;

Line 24-25:
  - inbox: Arc<Mutex<VecDeque<Vec<u8>>>>,
  - state: Arc<Mutex<PeerState>>,
  + inbox: Rc<RefCell<VecDeque<Vec<u8>>>>,
  + state: Rc<RefCell<PeerState>>,

Line 60-61:
  - let inbox = Arc::new(Mutex::new(VecDeque::new()));
  - let state = Arc::new(Mutex::new(PeerState::New));
  + let inbox = Rc::new(RefCell::new(VecDeque::new()));
  + let state = Rc::new(RefCell::new(PeerState::New));

Line 80:
  - *self.state.lock().unwrap() = PeerState::Connecting;
  + *self.state.borrow_mut() = PeerState::Connecting;

Line 113-114:
  - let inbox = Arc::clone(&self.inbox);
  - let state = Arc::clone(&self.state);
  + let inbox = Rc::clone(&self.inbox);
  + let state = Rc::clone(&self.state);

Line 117-118:
  - let inbox2 = Arc::clone(&inbox);
  - let state2 = Arc::clone(&state);
  + let inbox2 = Rc::clone(&inbox);
  + let state2 = Rc::clone(&state);

Line 123:
  - inbox2.lock().unwrap().push_back(s.into_bytes());
  + inbox2.borrow_mut().push_back(s.into_bytes());

Line 130:
  - *state2.lock().unwrap() = PeerState::Connected;
  + *state2.borrow_mut() = PeerState::Connected;

Line 153:
  - *self.state.lock().unwrap() = PeerState::Connecting;
  + *self.state.borrow_mut() = PeerState::Connecting;

Line 195:
  - self.inbox.lock().unwrap().pop_front()
  + self.inbox.borrow_mut().pop_front()

Line 200:
  - format!("{:?}", *self.state.lock().unwrap())
  + format!("{:?}", *self.state.borrow())

Line 214:
  - *self.state.lock().unwrap() = PeerState::Disconnected;
  + *self.state.borrow_mut() = PeerState::Disconnected;

Line 220-221:
  - let inbox = Arc::clone(&self.inbox);
  - let state = Arc::clone(&self.state);
  + let inbox = Rc::clone(&self.inbox);
  + let state = Rc::clone(&self.state);

Line 226:
  - inbox.lock().unwrap().push_back(s.into_bytes());
  + inbox.borrow_mut().push_back(s.into_bytes());

Line 233:
  - *state.lock().unwrap() = PeerState::Connected;
  + *state.borrow_mut() = PeerState::Connected;
```

---

## Testing Commands

```bash
# Build for WASM
cd /Users/lagyeongjun/CD/SERVER/07_PROJECTS/333-platform
wasm-pack build --target bundler --release

# Check for size reduction
ls -lh pkg/triple_three_bg.wasm
# Before: ~XXX KB
# After: ~XXX KB (should be slightly smaller)

# If you have tests
wasm-pack test --release

# Check for panic messages in browser console
# Visit any page that uses the WASM
# DevTools → Console → Look for "panicked at"
```

---

## Common Mistakes During Migration

### ❌ Mistake 1: Forgetting to replace all Arc::clone
```rust
let inbox = Arc::clone(&self.inbox);  // ← Still Arc!
let callback = Closure::new(move || {
    inbox.borrow_mut();  // ← Type mismatch!
});
```

**Fix:** Search for all `Arc::clone` in the file
```bash
grep -n "Arc::clone" src/p2p/webrtc.rs
```

### ❌ Mistake 2: Holding borrow too long
```rust
let inbox_ref = self.inbox.borrow_mut();  // ← Borrow held
let callback = Closure::new(move || {
    inbox_ref.push_back(data);  // ← Can't move borrowed data into closure
});
```

**Fix:** Only borrow when needed, inside closure
```rust
let inbox = Rc::clone(&self.inbox);
let callback = Closure::new(move || {
    inbox.borrow_mut().push_back(data);  // ← Borrow scoped to closure
});
```

### ❌ Mistake 3: Not updating type alias
```rust
// channel.rs line 66 - Easy to miss!
type SharedQueue = Arc<Mutex<...>>;  // ← Still Arc!
```

**Fix:** Update the type alias
```rust
type SharedQueue = Rc<RefCell<...>>;
```

### ❌ Mistake 4: Mixing Arc and Rc
```rust
let inbox = Rc::new(RefCell::new(...));
let copy = Arc::clone(&inbox);  // ← Type error! Rc vs Arc
```

**Fix:** Match the smart pointer
```rust
let inbox = Rc::new(RefCell::new(...));
let copy = Rc::clone(&inbox);  // ← Correct
```

---

## Performance Verification

### Before Migration
```bash
# Build and measure
wasm-pack build --release
ls -lh pkg/triple_three_bg.wasm
# Example: triple_three_bg.wasm: 123 KB

# Profile in DevTools
# Performance tab → Start recording → Load page
# Look for: lock() calls, mutex overhead
```

### After Migration
```bash
# Build and measure
wasm-pack build --release
ls -lh pkg/triple_three_bg.wasm
# Expected: triple_three_bg.wasm: 120 KB (or less)

# Profile in DevTools
# Should see fewer syscall-like operations
# Message processing should be 10% faster
```

### Benchmarking (if test harness available)
```rust
#[wasm_bindgen_test]
fn bench_recv_100k() {
    let peer = WebRtcPeer::new().unwrap();
    
    // Pre-fill with messages
    for i in 0..100_000 {
        peer.inbox.borrow_mut().push_back(vec![i as u8]);
    }
    
    // Measure recv
    let start = web_sys::window()
        .unwrap()
        .performance()
        .unwrap()
        .now();
    
    for _ in 0..100_000 {
        let _ = peer.recv();
    }
    
    let elapsed = web_sys::window()
        .unwrap()
        .performance()
        .unwrap()
        .now() - start;
    
    console_log!("100K recv cycles: {:.2}ms", elapsed);
    // Expect: ~10-20ms with Rc<RefCell>
    // vs ~100-150ms with Arc<Mutex>
}
```

---

## Verification Checklist

After completing the migration, verify:

### Code Level
- [ ] `grep -r "Arc<" src/p2p/webrtc.rs` returns nothing
- [ ] `grep -r "Mutex<" src/p2p/webrtc.rs` returns nothing
- [ ] `grep -r "Arc<" src/p2p/channel.rs` returns nothing
- [ ] `grep -r "Mutex<" src/p2p/channel.rs` returns nothing
- [ ] `grep -r ".lock()" src/p2p/` returns nothing
- [ ] All `borrow_mut()` and `borrow()` calls have matching `Rc::clone`

### Build Level
- [ ] `wasm-pack build --release` succeeds
- [ ] No compiler warnings about unused Arc/Mutex
- [ ] Binary size is same or smaller than before

### Runtime Level
- [ ] Open page using the WASM module
- [ ] DevTools Console shows no panic messages
- [ ] WebRTC connection establishes successfully
- [ ] Messages send/receive correctly
- [ ] No "already borrowed" errors

### Performance Level
- [ ] Benchmark shows message processing is faster (or same)
- [ ] CPU usage during message processing is lower
- [ ] No frame drops in any related UI

---

## Rollback Plan

If something goes wrong:

```bash
# Undo changes
git checkout -- src/p2p/webrtc.rs src/p2p/channel.rs

# Or manual rollback
# 1. Restore imports: Arc, Mutex
# 2. Change type signatures back
# 3. Replace borrow() with lock().unwrap()
# 4. Replace borrow_mut() with lock().unwrap()
# 5. Replace Rc::clone with Arc::clone
```

---

## FAQ

**Q: Will my code compile?**
A: Yes. Rc<RefCell<T>> has the same API surface as Arc<Mutex<T>>, just without the Sync/Send traits.

**Q: Can I mix Arc and Rc?**
A: No. Either use all Rc or all Arc per struct. WASM is single-threaded, so Rc everywhere.

**Q: What about try_borrow() errors?**
A: They're BorrowError, not Option. Use `.map_err()` to convert to Result.

**Q: Will panics break my WASM?**
A: In browsers, panics are caught by JS and logged as errors. Your code won't crash the tab, but the specific closure will stop working.

**Q: How do I debug BorrowMutError panics?**
A: Add `console_error_panic_hook` crate and check browser console. Look for "already borrowed" panic messages.

**Q: Should I use try_borrow_mut() everywhere?**
A: No, just in hot paths or callbacks that might re-enter. Regular code can use borrow_mut() safely.

**Q: Is Rc<RefCell<T>> slower than Arc<Mutex<T>>?**
A: No, it's 10x faster because it has no atomic operations or lock overhead.

**Q: What's the binary size impact?**
A: ~2-5 KB reduction due to removing mutex/atomic code.

---

## Related KG References

- `KG: REFACTORING_333_Arc_to_Rc_migration_WASM` — Main guide
- `KG: CONTRACT_333_DataChannel` — DataChannel trait
- `KG: CONTRACT_333_MeshRoom` — Room management
- `KG: ATOM_Web_DataChannel` — Web implementation
- `KG: ATOM_Web_MeshRoom` — Web mesh room

---

## Next Steps

1. Read `WASM_REFACTORING_GUIDE.md` (comprehensive)
2. Review `MIGRATION_EXAMPLES.rs` (code patterns)
3. Execute migration in Priority order (1 → 2 → 3)
4. Test after each file
5. Verify all checks pass
6. Update KG with completion status

---

**Last Updated:** 2026-04-13
**Estimated Total Time:** 45-60 minutes
**Risk Level:** Low (wasm-specific change, no deps affected)
