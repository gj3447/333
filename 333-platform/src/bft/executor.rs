// KG: SPAN_333_BFT_Executor
//! Transaction Executor — execute committed OrderedTx
//!
//! After HotStuff commits a block, the executor:
//! 1. Validates each transaction (balance check, nonce, permissions)
//! 2. Applies state changes (transfer tokens, settle auctions)
//! 3. Returns execution results
//!
//! This is the "application layer" of BFT consensus.

use super::crypto::NodeId;
use super::types::OrderedTx;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Account state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub balance: u64,
    pub nonce: u64,
}

/// Execution result for a single transaction
#[derive(Debug, Clone)]
pub enum TxResult {
    Success,
    InsufficientBalance { available: u64, required: u64 },
    InvalidNonce { expected: u64, got: u64 },
    SelfTransfer,
}

/// State machine for executing ordered transactions
#[derive(Debug, Clone)]
pub struct Executor {
    accounts: HashMap<NodeId, Account>,
}

impl Executor {
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
        }
    }

    /// Initialize an account with a starting balance (for genesis / testing)
    pub fn init_account(&mut self, node_id: NodeId, balance: u64) {
        self.accounts.insert(
            node_id,
            Account { balance, nonce: 0 },
        );
    }

    /// Get account balance (0 if not exists)
    pub fn balance(&self, node_id: &NodeId) -> u64 {
        self.accounts.get(node_id).map_or(0, |a| a.balance)
    }

    /// Get account nonce
    pub fn nonce(&self, node_id: &NodeId) -> u64 {
        self.accounts.get(node_id).map_or(0, |a| a.nonce)
    }

    /// Execute a batch of committed transactions (from a committed block)
    pub fn execute_block(&mut self, txs: &[OrderedTx]) -> Vec<TxResult> {
        txs.iter().map(|tx| self.execute_one(tx)).collect()
    }

    /// Execute a single transaction
    fn execute_one(&mut self, tx: &OrderedTx) -> TxResult {
        match tx {
            OrderedTx::Transfer { from, to, amount, nonce } => {
                self.execute_transfer(*from, *to, *amount, *nonce)
            }
            OrderedTx::AuctionBid { bidder, item_id: _, amount } => {
                // Auction: lock tokens (deduct from bidder, hold in escrow)
                self.execute_transfer(*bidder, 0, *amount, self.nonce(bidder))
            }
            OrderedTx::RankedAction { player: _, action_type: _, payload: _ } => {
                // Ranked actions don't affect token state — just ordering
                TxResult::Success
            }
        }
    }

    fn execute_transfer(&mut self, from: NodeId, to: NodeId, amount: u64, nonce: u64) -> TxResult {
        if from == to {
            return TxResult::SelfTransfer;
        }

        // Check nonce
        let current_nonce = self.nonce(&from);
        if nonce != current_nonce {
            return TxResult::InvalidNonce {
                expected: current_nonce,
                got: nonce,
            };
        }

        // Check balance
        let balance = self.balance(&from);
        if balance < amount {
            return TxResult::InsufficientBalance {
                available: balance,
                required: amount,
            };
        }

        // Apply transfer
        self.accounts
            .entry(from)
            .and_modify(|a| {
                a.balance -= amount;
                a.nonce += 1;
            });

        self.accounts
            .entry(to)
            .or_insert(Account { balance: 0, nonce: 0 })
            .balance += amount;

        TxResult::Success
    }

    /// Total supply across all accounts (for invariant checking)
    pub fn total_supply(&self) -> u64 {
        self.accounts.values().map(|a| a.balance).sum()
    }

    /// Number of accounts
    pub fn account_count(&self) -> usize {
        self.accounts.len()
    }
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_transfer() {
        let mut exec = Executor::new();
        exec.init_account(1, 1000);
        exec.init_account(2, 500);

        let tx = OrderedTx::Transfer { from: 1, to: 2, amount: 300, nonce: 0 };
        let result = exec.execute_block(&[tx]);

        assert!(matches!(result[0], TxResult::Success));
        assert_eq!(exec.balance(&1), 700);
        assert_eq!(exec.balance(&2), 800);
    }

    #[test]
    fn insufficient_balance() {
        let mut exec = Executor::new();
        exec.init_account(1, 100);

        let tx = OrderedTx::Transfer { from: 1, to: 2, amount: 200, nonce: 0 };
        let result = exec.execute_block(&[tx]);

        assert!(matches!(result[0], TxResult::InsufficientBalance { .. }));
        assert_eq!(exec.balance(&1), 100, "balance unchanged on failure");
    }

    #[test]
    fn nonce_prevents_replay() {
        let mut exec = Executor::new();
        exec.init_account(1, 1000);

        // First transfer: nonce 0 → success
        let tx1 = OrderedTx::Transfer { from: 1, to: 2, amount: 100, nonce: 0 };
        exec.execute_block(&[tx1]);

        // Replay same nonce 0 → fail
        let tx2 = OrderedTx::Transfer { from: 1, to: 2, amount: 100, nonce: 0 };
        let result = exec.execute_block(&[tx2]);
        assert!(matches!(result[0], TxResult::InvalidNonce { expected: 1, got: 0 }));

        // Correct nonce 1 → success
        let tx3 = OrderedTx::Transfer { from: 1, to: 2, amount: 100, nonce: 1 };
        let result = exec.execute_block(&[tx3]);
        assert!(matches!(result[0], TxResult::Success));
    }

    #[test]
    fn self_transfer_rejected() {
        let mut exec = Executor::new();
        exec.init_account(1, 1000);

        let tx = OrderedTx::Transfer { from: 1, to: 1, amount: 100, nonce: 0 };
        let result = exec.execute_block(&[tx]);
        assert!(matches!(result[0], TxResult::SelfTransfer));
    }

    #[test]
    fn supply_conservation() {
        let mut exec = Executor::new();
        exec.init_account(1, 1000);
        exec.init_account(2, 500);
        let initial_supply = exec.total_supply();

        // Multiple transfers
        exec.execute_block(&[
            OrderedTx::Transfer { from: 1, to: 2, amount: 300, nonce: 0 },
            OrderedTx::Transfer { from: 2, to: 1, amount: 100, nonce: 0 },
        ]);

        assert_eq!(exec.total_supply(), initial_supply, "total supply must be conserved");
    }

    #[test]
    fn block_of_mixed_transactions() {
        let mut exec = Executor::new();
        exec.init_account(1, 10000);
        exec.init_account(2, 5000);

        let results = exec.execute_block(&[
            OrderedTx::Transfer { from: 1, to: 2, amount: 1000, nonce: 0 },
            OrderedTx::RankedAction { player: 1, action_type: 1, payload: vec![1, 2, 3] },
            OrderedTx::AuctionBid { bidder: 2, item_id: 42, amount: 500 },
        ]);

        assert!(matches!(results[0], TxResult::Success));
        assert!(matches!(results[1], TxResult::Success)); // RankedAction always succeeds
        assert!(matches!(results[2], TxResult::Success)); // Auction bid locks tokens

        assert_eq!(exec.balance(&1), 9000);
        assert_eq!(exec.balance(&2), 5500); // 5000 + 1000 - 500
    }

    #[test]
    fn new_account_created_on_receive() {
        let mut exec = Executor::new();
        exec.init_account(1, 1000);
        // Account 3 doesn't exist yet

        let tx = OrderedTx::Transfer { from: 1, to: 3, amount: 500, nonce: 0 };
        exec.execute_block(&[tx]);

        assert_eq!(exec.balance(&3), 500, "new account created with received amount");
        assert!(exec.account_count() >= 2, "at least sender + receiver accounts");
    }
}
