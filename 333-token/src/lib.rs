// KG: SPAN_333_L11plus_Token, plan-333-p2p-os-synthesis-execution-2026-04-18,
//     queue-p10-testnet-token-2026-04-18
//
// 333 P2P OS L11+ Token — ledger + staking primitives for the testnet economy.
//
// Scope:
//   - TokenLedger trait (balance / mint / burn / transfer) with conservation check.
//   - StakePool (lock funds for an epoch, slash on misbehaviour, unbond).
//   - EmissionCurve (per-epoch emission, halving every N epochs — inspired by
//     Bitcoin supply schedule, parameters are overridable).
//
// This crate is settlement-agnostic: a consensus backend applies ordered
// SettlementOp::Transfer events by calling `TokenLedger::transfer`. Reward
// distribution is done by the incentive module (333-incentive) which holds a
// reference to the ledger.
//
// Invariants:
//   - Total supply = Σ balances + Σ stakes is monotone under transfer/stake.
//   - Mint increases total supply; burn decreases it; nothing else shifts it.
//   - Slash moves stake → void (burn-equivalent), total supply strictly drops.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use identity333::NodeId;
use thiserror::Error;

pub type Amount = u128;
pub type Epoch = u64;

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum TokenError {
    #[error("insufficient balance: have {have} need {need}")]
    Insufficient { have: Amount, need: Amount },
    #[error("no stake for account")]
    NoStake,
    #[error("stake locked until epoch {0}")]
    StakeLocked(Epoch),
    #[error("zero amount not allowed")]
    ZeroAmount,
    #[error("emission: halving step too large, max 128")]
    BadHalving,
    #[error("overflow")]
    Overflow,
}

// ============================================================================
// TokenLedger trait
// ============================================================================

pub trait TokenLedger: Send + Sync {
    fn balance(&self, acct: &NodeId) -> Amount;
    fn mint(&self, acct: &NodeId, amount: Amount) -> Result<(), TokenError>;
    fn burn(&self, acct: &NodeId, amount: Amount) -> Result<(), TokenError>;
    fn transfer(&self, from: &NodeId, to: &NodeId, amount: Amount) -> Result<(), TokenError>;
    fn total_supply(&self) -> Amount;
}

// ============================================================================
// InMemoryLedger — reference impl
// ============================================================================

pub struct InMemoryLedger {
    inner: Mutex<LedgerInner>,
}

#[derive(Default)]
struct LedgerInner {
    balances: HashMap<NodeId, Amount>,
    total: Amount,
}

impl Default for InMemoryLedger {
    fn default() -> Self {
        Self { inner: Mutex::new(LedgerInner::default()) }
    }
}

impl InMemoryLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> HashMap<NodeId, Amount> {
        self.inner.lock().unwrap().balances.clone()
    }
}

impl TokenLedger for InMemoryLedger {
    fn balance(&self, acct: &NodeId) -> Amount {
        self.inner.lock().unwrap().balances.get(acct).copied().unwrap_or(0)
    }

    fn mint(&self, acct: &NodeId, amount: Amount) -> Result<(), TokenError> {
        if amount == 0 {
            return Err(TokenError::ZeroAmount);
        }
        let mut g = self.inner.lock().unwrap();
        let b = g.balances.entry(acct.clone()).or_insert(0);
        *b = b.checked_add(amount).ok_or(TokenError::Overflow)?;
        g.total = g.total.checked_add(amount).ok_or(TokenError::Overflow)?;
        Ok(())
    }

    fn burn(&self, acct: &NodeId, amount: Amount) -> Result<(), TokenError> {
        if amount == 0 {
            return Err(TokenError::ZeroAmount);
        }
        let mut g = self.inner.lock().unwrap();
        let b = g.balances.entry(acct.clone()).or_insert(0);
        if *b < amount {
            return Err(TokenError::Insufficient { have: *b, need: amount });
        }
        *b -= amount;
        g.total -= amount;
        Ok(())
    }

    fn transfer(&self, from: &NodeId, to: &NodeId, amount: Amount) -> Result<(), TokenError> {
        if amount == 0 {
            return Err(TokenError::ZeroAmount);
        }
        let mut g = self.inner.lock().unwrap();
        let from_b = g.balances.entry(from.clone()).or_insert(0);
        if *from_b < amount {
            return Err(TokenError::Insufficient { have: *from_b, need: amount });
        }
        *from_b -= amount;
        let to_b = g.balances.entry(to.clone()).or_insert(0);
        *to_b = to_b.checked_add(amount).ok_or(TokenError::Overflow)?;
        // total unchanged — transfer conserves supply
        Ok(())
    }

    fn total_supply(&self) -> Amount {
        self.inner.lock().unwrap().total
    }
}

// ============================================================================
// StakePool — lock/unbond/slash
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stake {
    pub amount: Amount,
    pub unlock_epoch: Epoch,
}

pub struct StakePool {
    inner: Mutex<StakeInner>,
}

#[derive(Default)]
struct StakeInner {
    stakes: BTreeMap<NodeId, Stake>,
    current_epoch: Epoch,
    slashed_total: Amount,
}

impl Default for StakePool {
    fn default() -> Self {
        Self { inner: Mutex::new(StakeInner::default()) }
    }
}

impl StakePool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_epoch(&self, e: Epoch) {
        self.inner.lock().unwrap().current_epoch = e;
    }

    pub fn current_epoch(&self) -> Epoch {
        self.inner.lock().unwrap().current_epoch
    }

    /// Lock `amount` from `ledger` into a stake unlocking at `unlock_epoch`.
    pub fn stake<L: TokenLedger>(
        &self,
        ledger: &L,
        acct: &NodeId,
        amount: Amount,
        unlock_epoch: Epoch,
    ) -> Result<(), TokenError> {
        if amount == 0 {
            return Err(TokenError::ZeroAmount);
        }
        ledger.burn(acct, amount)?; // funds leave circulation while staked
        let mut g = self.inner.lock().unwrap();
        let entry = g.stakes.entry(acct.clone()).or_insert(Stake {
            amount: 0,
            unlock_epoch,
        });
        entry.amount = entry
            .amount
            .checked_add(amount)
            .ok_or(TokenError::Overflow)?;
        // extend lock if new unlock is later
        if unlock_epoch > entry.unlock_epoch {
            entry.unlock_epoch = unlock_epoch;
        }
        Ok(())
    }

    /// Return staked funds to `ledger` once the lock has expired.
    pub fn unbond<L: TokenLedger>(&self, ledger: &L, acct: &NodeId) -> Result<Amount, TokenError> {
        let mut g = self.inner.lock().unwrap();
        let entry = g.stakes.get(acct).copied().ok_or(TokenError::NoStake)?;
        if entry.unlock_epoch > g.current_epoch {
            return Err(TokenError::StakeLocked(entry.unlock_epoch));
        }
        g.stakes.remove(acct);
        drop(g);
        ledger.mint(acct, entry.amount)?;
        Ok(entry.amount)
    }

    /// Slash a fraction of an account's stake. `numer/denom` of stake is burned.
    /// No ledger interaction — slashed funds are destroyed, not returned.
    pub fn slash(&self, acct: &NodeId, numer: u32, denom: u32) -> Result<Amount, TokenError> {
        assert!(denom > 0 && numer <= denom, "slash fraction must be ≤ 1");
        let mut g = self.inner.lock().unwrap();
        let entry = g.stakes.get_mut(acct).ok_or(TokenError::NoStake)?;
        let slashed = (entry.amount * numer as Amount) / denom as Amount;
        entry.amount -= slashed;
        let now_zero = entry.amount == 0;
        g.slashed_total += slashed;
        if now_zero {
            g.stakes.remove(acct);
        }
        Ok(slashed)
    }

    pub fn stake_of(&self, acct: &NodeId) -> Option<Stake> {
        self.inner.lock().unwrap().stakes.get(acct).copied()
    }

    pub fn total_staked(&self) -> Amount {
        self.inner.lock().unwrap().stakes.values().map(|s| s.amount).sum()
    }

    pub fn total_slashed(&self) -> Amount {
        self.inner.lock().unwrap().slashed_total
    }
}

// ============================================================================
// EmissionCurve — per-epoch reward that halves every `halving_interval`
// ============================================================================

#[derive(Debug, Clone, Copy)]
pub struct EmissionCurve {
    pub initial_reward: Amount,
    pub halving_interval: Epoch,
}

impl EmissionCurve {
    pub fn new(initial_reward: Amount, halving_interval: Epoch) -> Self {
        assert!(halving_interval > 0, "halving_interval must be positive");
        Self { initial_reward, halving_interval }
    }

    /// Reward at epoch `e` = initial >> (e / halving_interval), saturating to 0 past 128 halvings.
    pub fn reward_at(&self, e: Epoch) -> Result<Amount, TokenError> {
        let steps = (e / self.halving_interval) as u32;
        if steps >= 128 {
            return Ok(0);
        }
        Ok(self.initial_reward >> steps)
    }

    /// Sum of all rewards from epoch 0 to `last_epoch` inclusive.
    pub fn cumulative_through(&self, last_epoch: Epoch) -> Result<Amount, TokenError> {
        let mut total: Amount = 0;
        let mut e: Epoch = 0;
        while e <= last_epoch {
            let r = self.reward_at(e)?;
            total = total.checked_add(r).ok_or(TokenError::Overflow)?;
            e += 1;
            // short-circuit: reward is zero past this point
            if r == 0 {
                break;
            }
        }
        Ok(total)
    }
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use identity333::Keypair;

    fn node() -> NodeId {
        Keypair::generate().node_id()
    }

    // ---- Ledger ----

    #[test]
    fn mint_increases_balance_and_supply() {
        let l = InMemoryLedger::new();
        let a = node();
        l.mint(&a, 100).unwrap();
        assert_eq!(l.balance(&a), 100);
        assert_eq!(l.total_supply(), 100);
    }

    #[test]
    fn burn_decreases_balance_and_supply() {
        let l = InMemoryLedger::new();
        let a = node();
        l.mint(&a, 100).unwrap();
        l.burn(&a, 30).unwrap();
        assert_eq!(l.balance(&a), 70);
        assert_eq!(l.total_supply(), 70);
    }

    #[test]
    fn burn_over_balance_rejected() {
        let l = InMemoryLedger::new();
        let a = node();
        l.mint(&a, 5).unwrap();
        assert!(matches!(
            l.burn(&a, 10),
            Err(TokenError::Insufficient { .. })
        ));
    }

    #[test]
    fn transfer_conserves_supply() {
        let l = InMemoryLedger::new();
        let a = node();
        let b = node();
        l.mint(&a, 100).unwrap();
        let before = l.total_supply();
        l.transfer(&a, &b, 40).unwrap();
        assert_eq!(l.total_supply(), before);
        assert_eq!(l.balance(&a), 60);
        assert_eq!(l.balance(&b), 40);
    }

    #[test]
    fn transfer_rejects_zero() {
        let l = InMemoryLedger::new();
        let a = node();
        let b = node();
        l.mint(&a, 10).unwrap();
        assert!(matches!(l.transfer(&a, &b, 0), Err(TokenError::ZeroAmount)));
    }

    #[test]
    fn mint_zero_rejected() {
        let l = InMemoryLedger::new();
        let a = node();
        assert!(matches!(l.mint(&a, 0), Err(TokenError::ZeroAmount)));
    }

    // ---- Stake ----

    #[test]
    fn stake_burns_from_ledger_and_accrues_pool() {
        let l = InMemoryLedger::new();
        let pool = StakePool::new();
        let a = node();
        l.mint(&a, 1_000).unwrap();
        pool.stake(&l, &a, 400, 10).unwrap();
        assert_eq!(l.balance(&a), 600);
        assert_eq!(pool.total_staked(), 400);
    }

    #[test]
    fn unbond_requires_unlock_epoch_reached() {
        let l = InMemoryLedger::new();
        let pool = StakePool::new();
        let a = node();
        l.mint(&a, 1_000).unwrap();
        pool.stake(&l, &a, 200, 10).unwrap();
        pool.set_epoch(5);
        assert!(matches!(
            pool.unbond(&l, &a),
            Err(TokenError::StakeLocked(10))
        ));
        pool.set_epoch(10);
        let returned = pool.unbond(&l, &a).unwrap();
        assert_eq!(returned, 200);
        assert_eq!(l.balance(&a), 1_000);
    }

    #[test]
    fn slash_burns_fraction_without_ledger_touch() {
        let l = InMemoryLedger::new();
        let pool = StakePool::new();
        let a = node();
        l.mint(&a, 1_000).unwrap();
        pool.stake(&l, &a, 400, 10).unwrap();
        // Slash 1/4 of stake.
        let slashed = pool.slash(&a, 1, 4).unwrap();
        assert_eq!(slashed, 100);
        assert_eq!(pool.total_staked(), 300);
        assert_eq!(pool.total_slashed(), 100);
        // Ledger unaffected by slash — slashed funds were already burned on stake.
        assert_eq!(l.balance(&a), 600);
    }

    #[test]
    fn slash_full_removes_stake() {
        let l = InMemoryLedger::new();
        let pool = StakePool::new();
        let a = node();
        l.mint(&a, 100).unwrap();
        pool.stake(&l, &a, 100, 5).unwrap();
        pool.slash(&a, 1, 1).unwrap();
        assert!(pool.stake_of(&a).is_none());
        assert_eq!(pool.total_staked(), 0);
    }

    #[test]
    fn stake_zero_rejected() {
        let l = InMemoryLedger::new();
        let pool = StakePool::new();
        let a = node();
        l.mint(&a, 100).unwrap();
        assert!(matches!(pool.stake(&l, &a, 0, 5), Err(TokenError::ZeroAmount)));
    }

    #[test]
    fn unbond_no_stake_errors() {
        let l = InMemoryLedger::new();
        let pool = StakePool::new();
        let a = node();
        assert!(matches!(pool.unbond(&l, &a), Err(TokenError::NoStake)));
    }

    #[test]
    fn stake_extends_lock_on_second_deposit() {
        let l = InMemoryLedger::new();
        let pool = StakePool::new();
        let a = node();
        l.mint(&a, 1_000).unwrap();
        pool.stake(&l, &a, 100, 5).unwrap();
        pool.stake(&l, &a, 100, 20).unwrap();
        let s = pool.stake_of(&a).unwrap();
        assert_eq!(s.amount, 200);
        assert_eq!(s.unlock_epoch, 20);
    }

    // ---- Emission ----

    #[test]
    fn emission_halves_every_interval() {
        let curve = EmissionCurve::new(1_000, 10);
        assert_eq!(curve.reward_at(0).unwrap(), 1_000);
        assert_eq!(curve.reward_at(9).unwrap(), 1_000);
        assert_eq!(curve.reward_at(10).unwrap(), 500);
        assert_eq!(curve.reward_at(20).unwrap(), 250);
        assert_eq!(curve.reward_at(30).unwrap(), 125);
    }

    #[test]
    fn emission_saturates_to_zero_past_128_halvings() {
        let curve = EmissionCurve::new(1_000, 1);
        assert_eq!(curve.reward_at(1_000).unwrap(), 0);
    }

    #[test]
    fn emission_cumulative_bounded() {
        let curve = EmissionCurve::new(1_000, 10);
        // Σ 1000 × 10 + 500 × 10 + 250 × 10 + ... → geometric bounded by 2 × initial × interval = 20_000.
        let cum = curve.cumulative_through(1_000).unwrap();
        assert!(cum < 20_000);
        assert!(cum >= 10_000); // at least the first halving window's worth
    }

    #[test]
    fn ledger_snapshot_captures_state() {
        let l = InMemoryLedger::new();
        let a = node();
        let b = node();
        l.mint(&a, 50).unwrap();
        l.mint(&b, 70).unwrap();
        let snap = l.snapshot();
        assert_eq!(snap.get(&a).copied(), Some(50));
        assert_eq!(snap.get(&b).copied(), Some(70));
    }
}
