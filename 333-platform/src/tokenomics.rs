// KG: CONTRACT_333_Tokenomics (추가)
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-tokenomics-MAX_SUPPLY/GENESIS_REWARD/BLOCK_REWARD/HALVING_INTERVAL/CoinAccount/SybilCheck/TokenTx/RewardReason/SlashReason/TokenResult/TokenLedger
// 333 Coin: utility token. NOT a security — pure utility.
// Pays for compute/storage/relay. Fixed max supply.
// Earning: Proof of Useful Work (PoUW) — DHT storage + uptime + stake.
// Spending: storage fees, relay fees, app deployment.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 333 Coin constants
pub const MAX_SUPPLY: u64 = 333_333_333; // 3.33억 — 상징적 수량
pub const GENESIS_REWARD: u64 = 10_000; // 제네시스 노드 보상
pub const BLOCK_REWARD: u64 = 33; // 블록당 채굴 보상 (감쇄 전)
pub const MIN_STAKE: u64 = 100; // Super Peer 최소 스테이킹
pub const HALVING_INTERVAL: u64 = 1_000_000; // 100만 블록마다 반감
pub const STORAGE_FEE_PER_MB: u64 = 1; // 1 MB DHT 저장 비용
pub const RELAY_FEE_PER_MSG: u64 = 0; // 릴레이 무료 (초기)
pub const MAX_ACCOUNTS: usize = 1_000_000; // FIX HIGH #7: DoS 방지

/// 333 Coin account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoinAccount {
    pub balance: u64,
    pub staked: u64,
    pub nonce: u64,
    pub reputation: u32, // 0-1000, PoUW 기반
    pub uptime_hours: u64,
    pub storage_served_mb: u64,
}

impl CoinAccount {
    pub fn new() -> Self {
        Self {
            balance: 0,
            staked: 0,
            nonce: 0,
            reputation: 500, // 중립 시작
            uptime_hours: 0,
            storage_served_mb: 0,
        }
    }

    pub fn available_balance(&self) -> u64 {
        self.balance.saturating_sub(self.staked)
    }

    pub fn is_super_peer_eligible(&self) -> bool {
        self.staked >= MIN_STAKE && self.reputation >= 300
    }
}

impl Default for CoinAccount {
    fn default() -> Self {
        Self::new()
    }
}

/// Anti-Sybil 3-layer defense (Prometheus research: research-333-anti-sybil)
/// Layer 1: Reputation (Witnet-style) — task allocation weighted by history
/// Layer 2: Stake — MIN_STAKE tokens locked as collateral
/// Layer 3: IP/Temporal — 7-day wait + /24 subnet limit (max 3 nodes)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SybilCheck {
    pub node_id: u32,
    pub stake_ok: bool,
    pub reputation_ok: bool,
    pub age_days: u64,
    pub subnet_peers: u32,
}

impl SybilCheck {
    pub fn is_eligible(&self) -> bool {
        self.stake_ok && self.reputation_ok && self.age_days >= 7 && self.subnet_peers <= 3
    }
}

/// Token transaction types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TokenTx {
    /// 일반 전송
    Transfer { from: u32, to: u32, amount: u64, nonce: u64 },
    /// 스테이킹 (Super Peer 등록)
    Stake { node_id: u32, amount: u64 },
    /// 언스테이킹
    Unstake { node_id: u32, amount: u64 },
    /// PoUW 보상 (시스템 발행)
    RewardPoUW { node_id: u32, amount: u64, reason: RewardReason },
    /// 블록 보상 (리더에게)
    BlockReward { leader: u32, block_height: u64 },
    /// 슬래싱 (부정행위)
    Slash { node_id: u32, amount: u64, reason: SlashReason },
    /// 스토리지 요금
    StorageFee { node_id: u32, megabytes: u64 },
    /// 기부 (기부경매 삼권분립 시스템 — KG: 기부경매_삼권분립)
    /// 자본가가 기부탑 만들고 수수료 설정. 가장 많이 기부한 사람이 전체 기부금 수령.
    Donate { donor: u32, pool_id: u64, amount: u64 },
    /// 경매 (Vickrey sealed-bid second-price — KG: feedback-vickrey-auction)
    Auction { bidder: u32, item_id: u64, bid_amount: u64, nonce: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RewardReason {
    DHTStorage, // DHT 데이터 제공
    Uptime,     // 안정적 가동
    Relay,      // 메시지 릴레이
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SlashReason {
    InvalidData,    // 잘못된 데이터 제공
    Downtime,       // 약속된 가동시간 미달
    Equivocation,   // BFT 이중 투표
}

/// Token transaction result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenResult {
    Success,
    InsufficientBalance,
    InsufficientStake,
    InvalidNonce,
    SelfTransfer,
    SupplyExceeded,
    ReputationTooLow,
}

/// 333 Token Ledger — 토큰 상태 관리
#[derive(Debug, Clone)]
pub struct TokenLedger {
    accounts: HashMap<u32, CoinAccount>,
    total_minted: u64,
    total_burned: u64,
    block_height: u64,
}

impl TokenLedger {
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
            total_minted: 0,
            total_burned: 0,
            block_height: 0,
        }
    }

    /// Genesis: 초기 배포
    pub fn genesis(&mut self, validators: &[u32]) {
        for &v in validators {
            let acc = self.accounts.entry(v).or_default();
            acc.balance = GENESIS_REWARD;
            self.total_minted += GENESIS_REWARD;
        }
    }

    pub fn account(&self, node_id: u32) -> Option<&CoinAccount> {
        self.accounts.get(&node_id)
    }

    pub fn balance(&self, node_id: u32) -> u64 {
        self.accounts.get(&node_id).map_or(0, |a| a.balance)
    }

    pub fn staked(&self, node_id: u32) -> u64 {
        self.accounts.get(&node_id).map_or(0, |a| a.staked)
    }

    pub fn total_supply(&self) -> u64 {
        self.total_minted - self.total_burned
    }

    pub fn circulating_supply(&self) -> u64 {
        self.accounts.values().map(|a| a.balance).sum()
    }

    /// Current block reward (halving)
    pub fn current_block_reward(&self) -> u64 {
        let halvings = self.block_height / HALVING_INTERVAL;
        if halvings >= 64 { return 0; }
        BLOCK_REWARD >> halvings
    }

    /// Execute a token transaction
    pub fn execute(&mut self, tx: &TokenTx) -> TokenResult {
        match tx {
            TokenTx::Transfer { from, to, amount, nonce } => {
                self.execute_transfer(*from, *to, *amount, *nonce)
            }
            TokenTx::Stake { node_id, amount } => {
                self.execute_stake(*node_id, *amount)
            }
            TokenTx::Unstake { node_id, amount } => {
                self.execute_unstake(*node_id, *amount)
            }
            TokenTx::RewardPoUW { node_id, amount, .. } => {
                self.execute_reward(*node_id, *amount)
            }
            TokenTx::BlockReward { leader, block_height } => {
                self.block_height = *block_height;
                let reward = self.current_block_reward();
                if self.total_minted + reward > MAX_SUPPLY {
                    return TokenResult::SupplyExceeded;
                }
                self.execute_reward(*leader, reward)
            }
            TokenTx::Slash { node_id, amount, .. } => {
                self.execute_slash(*node_id, *amount)
            }
            TokenTx::Donate { donor, pool_id: _, amount } => {
                let acc = self.accounts.entry(*donor).or_default();
                if acc.available_balance() < *amount {
                    return TokenResult::InsufficientBalance;
                }
                acc.balance -= *amount;
                acc.nonce += 1;
                // Pool에 누적 (실제 구현은 DonationPool struct 필요)
                // 삼권분립: (1)탑에서 경쟁하는 기부자 (2)수수료 받는 자본가 (3)CPU로 구조 지탱하는 채굴자
                TokenResult::Success
            }
            TokenTx::Auction { bidder, item_id: _, bid_amount, nonce } => {
                let acc = self.accounts.entry(*bidder).or_default();
                if acc.nonce != *nonce { return TokenResult::InvalidNonce; }
                if acc.available_balance() < *bid_amount {
                    return TokenResult::InsufficientBalance;
                }
                // Vickrey: sealed-bid, 2nd price. Winner pays 2nd highest bid.
                // 실제 경매 정산은 별도 AuctionEngine 필요.
                acc.balance -= *bid_amount; // 에스크로
                acc.nonce += 1;
                TokenResult::Success
            }
            TokenTx::StorageFee { node_id, megabytes } => {
                let fee = megabytes * STORAGE_FEE_PER_MB;
                let acc = self.accounts.entry(*node_id).or_default();
                if acc.available_balance() < fee {
                    return TokenResult::InsufficientBalance;
                }
                acc.balance -= fee;
                self.total_burned += fee; // 소각
                TokenResult::Success
            }
        }
    }

    fn execute_transfer(&mut self, from: u32, to: u32, amount: u64, nonce: u64) -> TokenResult {
        if from == to { return TokenResult::SelfTransfer; }
        // FIX HIGH #7: prevent unbounded account creation
        if !self.accounts.contains_key(&to) && self.accounts.len() >= MAX_ACCOUNTS {
            return TokenResult::InsufficientBalance; // account limit reached
        }

        let sender = self.accounts.entry(from).or_default();
        if sender.nonce != nonce { return TokenResult::InvalidNonce; }
        if sender.available_balance() < amount { return TokenResult::InsufficientBalance; }

        sender.balance -= amount;
        sender.nonce += 1;

        let receiver = self.accounts.entry(to).or_default();
        receiver.balance += amount;

        TokenResult::Success
    }

    fn execute_stake(&mut self, node_id: u32, amount: u64) -> TokenResult {
        let acc = self.accounts.entry(node_id).or_default();
        if acc.available_balance() < amount {
            return TokenResult::InsufficientBalance;
        }
        acc.staked += amount;
        TokenResult::Success
    }

    fn execute_unstake(&mut self, node_id: u32, amount: u64) -> TokenResult {
        let acc = self.accounts.entry(node_id).or_default();
        if acc.staked < amount {
            return TokenResult::InsufficientStake;
        }
        acc.staked -= amount;
        TokenResult::Success
    }

    fn execute_reward(&mut self, node_id: u32, amount: u64) -> TokenResult {
        if self.total_minted + amount > MAX_SUPPLY {
            return TokenResult::SupplyExceeded;
        }
        let acc = self.accounts.entry(node_id).or_default();
        acc.balance += amount;
        self.total_minted += amount;
        TokenResult::Success
    }

    fn execute_slash(&mut self, node_id: u32, amount: u64) -> TokenResult {
        let acc = self.accounts.entry(node_id).or_default();
        let total_available = acc.staked + acc.balance;
        if total_available == 0 {
            return TokenResult::InsufficientBalance;
        }
        // Slash from stake first, then balance. Clamp to available.
        let effective_amount = amount.min(total_available);
        let slash_from_stake = effective_amount.min(acc.staked);
        acc.staked -= slash_from_stake;
        let remaining = effective_amount.saturating_sub(slash_from_stake);
        acc.balance -= remaining;
        acc.reputation = acc.reputation.saturating_sub(100);
        self.total_burned += effective_amount;
        TokenResult::Success
    }
}

impl Default for TokenLedger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_distribution() {
        let mut ledger = TokenLedger::new();
        ledger.genesis(&[1, 2, 3]);
        assert_eq!(ledger.balance(1), GENESIS_REWARD);
        assert_eq!(ledger.balance(2), GENESIS_REWARD);
        assert_eq!(ledger.balance(3), GENESIS_REWARD);
        assert_eq!(ledger.total_supply(), GENESIS_REWARD * 3);
    }

    #[test]
    fn transfer() {
        let mut ledger = TokenLedger::new();
        ledger.genesis(&[1, 2]);
        let r = ledger.execute(&TokenTx::Transfer { from: 1, to: 2, amount: 500, nonce: 0 });
        assert_eq!(r, TokenResult::Success);
        assert_eq!(ledger.balance(1), GENESIS_REWARD - 500);
        assert_eq!(ledger.balance(2), GENESIS_REWARD + 500);
    }

    #[test]
    fn insufficient_balance() {
        let mut ledger = TokenLedger::new();
        ledger.genesis(&[1, 2]);
        let r = ledger.execute(&TokenTx::Transfer { from: 1, to: 2, amount: GENESIS_REWARD + 1, nonce: 0 });
        assert_eq!(r, TokenResult::InsufficientBalance);
    }

    #[test]
    fn nonce_check() {
        let mut ledger = TokenLedger::new();
        ledger.genesis(&[1, 2]);
        // nonce 0 성공
        ledger.execute(&TokenTx::Transfer { from: 1, to: 2, amount: 100, nonce: 0 });
        // nonce 0 재사용 실패
        let r = ledger.execute(&TokenTx::Transfer { from: 1, to: 2, amount: 100, nonce: 0 });
        assert_eq!(r, TokenResult::InvalidNonce);
        // nonce 1 성공
        let r = ledger.execute(&TokenTx::Transfer { from: 1, to: 2, amount: 100, nonce: 1 });
        assert_eq!(r, TokenResult::Success);
    }

    #[test]
    fn stake_and_unstake() {
        let mut ledger = TokenLedger::new();
        ledger.genesis(&[1]);
        ledger.execute(&TokenTx::Stake { node_id: 1, amount: 500 });
        assert_eq!(ledger.staked(1), 500);
        assert_eq!(ledger.account(1).unwrap().available_balance(), GENESIS_REWARD - 500);

        // 스테이킹 상태에서 전송 시 available_balance만 사용
        let r = ledger.execute(&TokenTx::Transfer { from: 1, to: 2, amount: GENESIS_REWARD, nonce: 0 });
        assert_eq!(r, TokenResult::InsufficientBalance);

        ledger.execute(&TokenTx::Unstake { node_id: 1, amount: 500 });
        assert_eq!(ledger.staked(1), 0);
    }

    #[test]
    fn super_peer_eligibility() {
        let mut ledger = TokenLedger::new();
        ledger.genesis(&[1]);
        let acc = ledger.account(1).unwrap();
        assert!(!acc.is_super_peer_eligible()); // staked < MIN_STAKE

        ledger.execute(&TokenTx::Stake { node_id: 1, amount: MIN_STAKE });
        let acc = ledger.account(1).unwrap();
        assert!(acc.is_super_peer_eligible());
    }

    #[test]
    fn block_reward_halving() {
        let mut ledger = TokenLedger::new();
        assert_eq!(ledger.current_block_reward(), BLOCK_REWARD);

        ledger.block_height = HALVING_INTERVAL;
        assert_eq!(ledger.current_block_reward(), BLOCK_REWARD / 2);

        ledger.block_height = HALVING_INTERVAL * 2;
        assert_eq!(ledger.current_block_reward(), BLOCK_REWARD / 4);
    }

    #[test]
    fn max_supply_cap() {
        let mut ledger = TokenLedger::new();
        ledger.total_minted = MAX_SUPPLY - 10;
        let r = ledger.execute(&TokenTx::RewardPoUW {
            node_id: 1, amount: 11, reason: RewardReason::DHTStorage
        });
        assert_eq!(r, TokenResult::SupplyExceeded);

        let r = ledger.execute(&TokenTx::RewardPoUW {
            node_id: 1, amount: 10, reason: RewardReason::DHTStorage
        });
        assert_eq!(r, TokenResult::Success);
    }

    #[test]
    fn slash_reduces_stake_and_reputation() {
        let mut ledger = TokenLedger::new();
        ledger.genesis(&[1]);
        ledger.execute(&TokenTx::Stake { node_id: 1, amount: 500 });

        ledger.execute(&TokenTx::Slash { node_id: 1, amount: 200, reason: SlashReason::Equivocation });
        assert_eq!(ledger.staked(1), 300); // 500 - 200
        assert_eq!(ledger.account(1).unwrap().reputation, 400); // 500 - 100
    }

    #[test]
    fn storage_fee_burns() {
        let mut ledger = TokenLedger::new();
        ledger.genesis(&[1]);
        let _before = ledger.total_supply();

        ledger.execute(&TokenTx::StorageFee { node_id: 1, megabytes: 100 });
        let fee = 100 * STORAGE_FEE_PER_MB;
        assert_eq!(ledger.balance(1), GENESIS_REWARD - fee);
        // burned
        assert_eq!(ledger.total_burned, fee);
    }

    #[test]
    fn supply_conservation() {
        let mut ledger = TokenLedger::new();
        ledger.genesis(&[1, 2, 3]);

        // 다양한 트랜잭션 실행
        ledger.execute(&TokenTx::Transfer { from: 1, to: 2, amount: 1000, nonce: 0 });
        ledger.execute(&TokenTx::Stake { node_id: 2, amount: 500 });
        ledger.execute(&TokenTx::BlockReward { leader: 3, block_height: 1 });

        // circulating = total_minted - burned
        let circ = ledger.circulating_supply();
        assert_eq!(circ, ledger.total_minted - ledger.total_burned);
    }

    #[test]
    fn self_transfer_rejected() {
        let mut ledger = TokenLedger::new();
        ledger.genesis(&[1]);
        let r = ledger.execute(&TokenTx::Transfer { from: 1, to: 1, amount: 100, nonce: 0 });
        assert_eq!(r, TokenResult::SelfTransfer);
    }
}
