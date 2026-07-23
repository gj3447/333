// KG: SA_333_Platform
//
// Owner-authentication boundary for the Byzantine-safe transfer rail.
//
// `AccountId` is currently a human-readable string (for example `alice`), so a
// public key carried by an untrusted order cannot prove ownership of that name.
// The verifier-owned `OwnerRegistry` is therefore the trust root for this
// vertical slice. A future migration may replace aliases with self-certifying
// identity333 NodeIds; until then both sender and recipient must be registered.

use std::collections::BTreeMap;
use std::fmt;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::{AccountId, Transfer};

const OWNER_INTENT_DOMAIN: &[u8] = b"transfer333/owner-intent/v2\0";
const POLICY_ID_DOMAIN: &[u8] = b"transfer333/owner-policy-id/v1\0";
const ORDER_ID_DOMAIN: &[u8] = b"transfer333/order-id/v2\0";
pub const MAX_NETWORK_ID_BYTES: usize = 128;
pub const MAX_ACCOUNT_ID_BYTES: usize = 256;

/// Canonical digest of `(NetworkId, sorted AccountId -> owner key roster)`.
/// It prevents the same human-readable alias from silently resolving to a
/// different key under another process's configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PolicyId([u8; 32]);

impl PolicyId {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for PolicyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Deployment/genesis/asset domain. Owner intents are not replayable between
/// different `NetworkId`s even when an account key and sequence are reused.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NetworkId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkIdError {
    Empty,
    TooLong { bytes: usize, max: usize },
}

impl NetworkId {
    pub fn new(value: impl Into<String>) -> Result<Self, NetworkIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(NetworkIdError::Empty);
        }
        if value.len() > MAX_NETWORK_ID_BYTES {
            return Err(NetworkIdError::TooLong {
                bytes: value.len(),
                max: MAX_NETWORK_ID_BYTES,
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NetworkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Immutable alias-to-key binding owned by the verifier/control plane.
#[derive(Debug, Clone)]
pub struct OwnerRegistry {
    keys: BTreeMap<AccountId, VerifyingKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerRegistryError {
    Empty,
    EmptyAccount,
    AccountTooLong { account: AccountId, bytes: usize, max: usize },
    DuplicateAccount { account: AccountId },
}

impl OwnerRegistry {
    pub fn new<I, S>(bindings: I) -> Result<Self, OwnerRegistryError>
    where
        I: IntoIterator<Item = (S, VerifyingKey)>,
        S: Into<AccountId>,
    {
        let mut keys = BTreeMap::new();
        for (account, key) in bindings {
            let account = account.into();
            if account.is_empty() {
                return Err(OwnerRegistryError::EmptyAccount);
            }
            if account.len() > MAX_ACCOUNT_ID_BYTES {
                return Err(OwnerRegistryError::AccountTooLong {
                    bytes: account.len(),
                    max: MAX_ACCOUNT_ID_BYTES,
                    account,
                });
            }
            if keys.insert(account.clone(), key).is_some() {
                return Err(OwnerRegistryError::DuplicateAccount { account });
            }
        }
        if keys.is_empty() {
            return Err(OwnerRegistryError::Empty);
        }
        Ok(Self { keys })
    }

    pub fn contains(&self, account: &AccountId) -> bool {
        self.keys.contains_key(account)
    }

    pub fn key_of(&self, account: &AccountId) -> Option<&VerifyingKey> {
        self.keys.get(account)
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&AccountId, &VerifyingKey)> {
        self.keys.iter()
    }
}

/// Immutable verification policy shared by authorities, collectors, and
/// certificate-verifying ledgers.
#[derive(Debug, Clone)]
pub struct TransferPolicy {
    network_id: NetworkId,
    owners: OwnerRegistry,
    id: PolicyId,
}

impl TransferPolicy {
    pub fn new(network_id: NetworkId, owners: OwnerRegistry) -> Self {
        let id = policy_id(&network_id, &owners);
        Self {
            network_id,
            owners,
            id,
        }
    }

    pub fn network_id(&self) -> &NetworkId {
        &self.network_id
    }

    pub fn owners(&self) -> &OwnerRegistry {
        &self.owners
    }

    pub fn id(&self) -> PolicyId {
        self.id
    }

    pub fn validate(&self, order: &SignedTransfer) -> Result<(), OwnerAuthError> {
        if order.network_id != self.network_id {
            return Err(OwnerAuthError::WrongNetwork {
                expected: self.network_id.clone(),
                got: order.network_id.clone(),
            });
        }
        if order.policy_id != self.id {
            return Err(OwnerAuthError::WrongPolicy {
                expected: self.id,
                got: order.policy_id,
            });
        }
        let transfer = &order.transfer;
        let owner_key = self
            .owners
            .key_of(&transfer.from)
            .ok_or_else(|| OwnerAuthError::UnknownSender {
                account: transfer.from.clone(),
            })?;
        if !self.owners.contains(&transfer.to) {
            return Err(OwnerAuthError::UnknownRecipient {
                account: transfer.to.clone(),
            });
        }
        owner_key
            .verify_strict(
                &owner_signing_message(
                    &order.policy_id,
                    &order.network_id,
                    transfer,
                    order.round,
                ),
                &order.owner_signature,
            )
            .map_err(|_| OwnerAuthError::InvalidOwnerSignature {
                account: transfer.from.clone(),
            })
    }
}

/// A network-bound transfer payload plus the registered owner's Ed25519 proof.
///
/// This is the only order type accepted by the authority/network/certificate
/// rail. The legacy bare `Transfer` remains confined to the explicitly
/// honest-owner local-credit `Replica` path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedTransfer {
    pub network_id: NetworkId,
    policy_id: PolicyId,
    pub transfer: Transfer,
    /// Owner-declared re-vote round for the (account, seq) slot. Round 0 is the
    /// initial attempt; an owner that caused a split vote re-signs at a higher
    /// round to open a fresh authority vote slot for the same seq (Linera-style
    /// rounds, consensusless stage-1 recovery). The round is inside the signed
    /// bytes, so it cannot be rewritten without the owner key, and it is part of
    /// the order's identity: same transfer at a different round is a different
    /// order.
    pub round: u64,
    owner_signature: Signature,
}

impl SignedTransfer {
    /// Sign an initial (round 0) order. See [`SignedTransfer::sign_at_round`].
    pub fn sign(policy: &TransferPolicy, transfer: Transfer, key: &SigningKey) -> Self {
        Self::sign_at_round(policy, transfer, 0, key)
    }

    /// Sign an order at an explicit round. A higher round opens a fresh vote
    /// slot for the same (account, seq), which is the recovery path from an
    /// equivocation split vote.
    pub fn sign_at_round(
        policy: &TransferPolicy,
        transfer: Transfer,
        round: u64,
        key: &SigningKey,
    ) -> Self {
        let network_id = policy.network_id().clone();
        let policy_id = policy.id();
        let owner_signature = key.sign(&owner_signing_message(
            &policy_id,
            &network_id,
            &transfer,
            round,
        ));
        Self {
            network_id,
            policy_id,
            transfer,
            round,
            owner_signature,
        }
    }

    /// Construct an untrusted decoded proof. Callers must still validate it
    /// against a verifier-owned `TransferPolicy` before any state mutation.
    pub fn from_parts(
        network_id: NetworkId,
        policy_id: PolicyId,
        transfer: Transfer,
        owner_signature: Signature,
    ) -> Self {
        Self::from_parts_at_round(network_id, policy_id, transfer, 0, owner_signature)
    }

    /// [`SignedTransfer::from_parts`] at an explicit decoded round.
    pub fn from_parts_at_round(
        network_id: NetworkId,
        policy_id: PolicyId,
        transfer: Transfer,
        round: u64,
        owner_signature: Signature,
    ) -> Self {
        Self {
            network_id,
            policy_id,
            transfer,
            round,
            owner_signature,
        }
    }

    pub fn owner_signature(&self) -> &Signature {
        &self.owner_signature
    }

    pub fn policy_id(&self) -> PolicyId {
        self.policy_id
    }

    /// Stable telemetry identity for one exact signed order. Unlike the plain
    /// `from:seq:to:amount` rendering, this distinguishes forged and legitimate
    /// proofs for the same payload without logging signature bytes.
    pub fn order_id(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(ORDER_ID_DOMAIN);
        hasher.update(self.policy_id.as_bytes());
        let mut body = Vec::new();
        put_network_transfer_body(&mut body, &self.network_id, &self.transfer, self.round);
        hasher.update(&body);
        hasher.update(self.owner_signature.to_bytes());
        hasher.finalize().into()
    }

    pub fn verify(&self, policy: &TransferPolicy) -> Result<(), OwnerAuthError> {
        policy.validate(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerAuthError {
    WrongNetwork { expected: NetworkId, got: NetworkId },
    WrongPolicy { expected: PolicyId, got: PolicyId },
    UnknownSender { account: AccountId },
    UnknownRecipient { account: AccountId },
    InvalidOwnerSignature { account: AccountId },
}

fn hash_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn policy_id(network_id: &NetworkId, owners: &OwnerRegistry) -> PolicyId {
    let mut hasher = Sha256::new();
    hasher.update(POLICY_ID_DOMAIN);
    hash_bytes(&mut hasher, network_id.as_str().as_bytes());
    for (account, key) in owners.iter() {
        hash_bytes(&mut hasher, account.as_bytes());
        hasher.update(key.as_bytes());
    }
    PolicyId(hasher.finalize().into())
}

fn put_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    buf.extend_from_slice(bytes);
}

/// Canonical network + transfer bytes shared by owner and authority domains.
/// The round follows `from_seq` so the owner re-vote round is inside every
/// signature preimage that covers the transfer.
pub(crate) fn put_network_transfer_body(
    buf: &mut Vec<u8>,
    network_id: &NetworkId,
    transfer: &Transfer,
    round: u64,
) {
    put_bytes(buf, network_id.as_str().as_bytes());
    put_bytes(buf, transfer.from.as_bytes());
    buf.extend_from_slice(&transfer.from_seq.to_le_bytes());
    buf.extend_from_slice(&round.to_le_bytes());
    put_bytes(buf, transfer.to.as_bytes());
    buf.extend_from_slice(&transfer.amount.to_le_bytes());
}

/// Domain-separated canonical bytes signed by an account owner.
pub fn owner_signing_message(
    policy_id: &PolicyId,
    network_id: &NetworkId,
    transfer: &Transfer,
    round: u64,
) -> Vec<u8> {
    let mut message = Vec::with_capacity(
        OWNER_INTENT_DOMAIN.len()
            + 48
            + network_id.as_str().len()
            + transfer.from.len()
            + transfer.to.len(),
    );
    message.extend_from_slice(OWNER_INTENT_DOMAIN);
    message.extend_from_slice(policy_id.as_bytes());
    put_network_transfer_body(&mut message, network_id, transfer, round);
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn policy() -> TransferPolicy {
        TransferPolicy::new(
            NetworkId::new("testnet").unwrap(),
            OwnerRegistry::new([
                ("alice", key(42).verifying_key()),
                ("bob", key(43).verifying_key()),
            ])
            .unwrap(),
        )
    }

    fn transfer() -> Transfer {
        Transfer {
            from: "alice".into(),
            from_seq: 0,
            to: "bob".into(),
            amount: 30,
        }
    }

    #[test]
    fn valid_owner_proof_is_network_and_account_bound() {
        let policy = policy();
        let order = SignedTransfer::sign(&policy, transfer(), &key(42));
        assert_eq!(order.verify(&policy), Ok(()));
    }

    #[test]
    fn registry_rejects_empty_and_duplicate_bindings() {
        let empty: Vec<(String, VerifyingKey)> = Vec::new();
        assert!(matches!(
            OwnerRegistry::new(empty),
            Err(OwnerRegistryError::Empty)
        ));
        assert!(matches!(
            OwnerRegistry::new([
                ("alice", key(42).verifying_key()),
                ("alice", key(43).verifying_key()),
            ]),
            Err(OwnerRegistryError::DuplicateAccount { account }) if account == "alice"
        ));
    }

    #[test]
    fn mutation_and_wrong_network_invalidate_owner_proof() {
        let policy = policy();
        let order = SignedTransfer::sign(&policy, transfer(), &key(42));
        let mut amount = order.clone();
        amount.transfer.amount += 1;
        assert!(matches!(
            amount.verify(&policy),
            Err(OwnerAuthError::InvalidOwnerSignature { .. })
        ));

        let other = TransferPolicy::new(
            NetworkId::new("othernet").unwrap(),
            policy.owners().clone(),
        );
        assert!(matches!(
            order.verify(&other),
            Err(OwnerAuthError::WrongNetwork { .. })
        ));
    }

    #[test]
    fn roster_drift_changes_policy_id_and_rejects_alias_rebinding() {
        let original = policy();
        let order = SignedTransfer::sign(&original, transfer(), &key(42));
        let rebound = TransferPolicy::new(
            original.network_id().clone(),
            OwnerRegistry::new([
                ("alice", key(42).verifying_key()),
                ("bob", key(99).verifying_key()),
            ])
            .unwrap(),
        );

        assert_ne!(original.id(), rebound.id());
        assert!(matches!(
            order.verify(&rebound),
            Err(OwnerAuthError::WrongPolicy { .. })
        ));
    }
}
