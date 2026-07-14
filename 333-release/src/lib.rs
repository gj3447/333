// KG: SPAN_333_Linf_Release, plan-333-p2p-os-synthesis-execution-2026-04-18,
//     queue-p13-public-release-2026-04-18
//
// 333 P2P OS Release — the data layer beneath docs site / signed artifacts /
// CDN distribution. This crate does NOT run a web server, publish to a CDN,
// or build a site — those are ops concerns delivered by external tooling.
// It DOES define:
//
//   - `ReleaseManifest`:  version, timestamp, artifacts (name + CID + size).
//   - `ArtifactSigner`:   trait + ed25519 impl via identity333.
//   - `ChecksumVerifier`: trait + content-addressed impl via content333::Cid.
//   - `ProvenanceChain`:  parent→child releases with monotone semver.
//
// Semver rule: child.version > parent.version, where comparison is
// (major, minor, patch) lexicographic. A chain enforces no regressions.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use content333::Cid;
use identity333::{Keypair, NodeId, Signature};
use thiserror::Error;

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, Error)]
pub enum ReleaseError {
    #[error("semver regression: child {child:?} ≤ parent {parent:?}")]
    SemverRegression { child: Version, parent: Version },
    #[error("artifact missing: {0}")]
    ArtifactMissing(String),
    #[error("checksum mismatch for {name}: expected {expected} got {got}")]
    ChecksumMismatch { name: String, expected: Cid, got: Cid },
    #[error("signature invalid")]
    BadSignature,
    #[error("unknown signer: {0:?}")]
    UnknownSigner(NodeId),
    #[error("release id collision: {0}")]
    DuplicateRelease(String),
    #[error("parent release not in chain: {0}")]
    UnknownParent(String),
    #[error("version parse error: {0}")]
    BadVersion(String),
}

// ============================================================================
// Version (semver)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self { major, minor, patch }
    }

    pub fn parse(s: &str) -> Result<Self, ReleaseError> {
        let parts: Vec<&str> = s.trim_start_matches('v').split('.').collect();
        if parts.len() != 3 {
            return Err(ReleaseError::BadVersion(s.into()));
        }
        let major: u32 = parts[0].parse().map_err(|_| ReleaseError::BadVersion(s.into()))?;
        let minor: u32 = parts[1].parse().map_err(|_| ReleaseError::BadVersion(s.into()))?;
        let patch: u32 = parts[2].parse().map_err(|_| ReleaseError::BadVersion(s.into()))?;
        Ok(Self { major, minor, patch })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{}.{}.{}", self.major, self.minor, self.patch)
    }
}

// ============================================================================
// Artifact
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub name: String,
    pub cid: Cid,
    pub size_bytes: u64,
}

// ============================================================================
// ReleaseManifest
// ============================================================================

#[derive(Debug, Clone)]
pub struct ReleaseManifest {
    pub release_id: String,
    pub version: Version,
    pub created_at_unix: u64,
    pub parent_release_id: Option<String>,
    pub artifacts: Vec<Artifact>,
    pub signer: NodeId,
    pub signature: Signature,
}

impl ReleaseManifest {
    /// Canonical byte representation used for signing and hashing. Order is
    /// fixed so every node hashing the same manifest converges on the same
    /// signature input.
    pub fn canonical_bytes(
        release_id: &str,
        version: Version,
        created_at_unix: u64,
        parent: Option<&str>,
        artifacts: &[Artifact],
        signer: &NodeId,
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(release_id.as_bytes());
        out.push(0);
        out.extend_from_slice(&version.major.to_be_bytes());
        out.extend_from_slice(&version.minor.to_be_bytes());
        out.extend_from_slice(&version.patch.to_be_bytes());
        out.extend_from_slice(&created_at_unix.to_be_bytes());
        if let Some(p) = parent {
            out.extend_from_slice(p.as_bytes());
        }
        out.push(0);
        // Artifacts: sort by name so input order doesn't affect the hash.
        let mut sorted: Vec<&Artifact> = artifacts.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        for a in sorted {
            out.extend_from_slice(a.name.as_bytes());
            out.push(0);
            out.extend_from_slice(a.cid.as_bytes());
            out.extend_from_slice(&a.size_bytes.to_be_bytes());
        }
        out.push(0);
        out.extend_from_slice(signer.as_bytes());
        out
    }
}

// ============================================================================
// ArtifactSigner trait
// ============================================================================

pub trait ArtifactSigner: Send + Sync {
    fn sign_manifest(
        &self,
        release_id: &str,
        version: Version,
        created_at_unix: u64,
        parent: Option<&str>,
        artifacts: Vec<Artifact>,
    ) -> Result<ReleaseManifest, ReleaseError>;

    fn verify_manifest(&self, m: &ReleaseManifest) -> Result<(), ReleaseError>;
}

/// Reference impl: signs with the bundled keypair and verifies against the
/// embedded signer's public key.
pub struct Ed25519Signer {
    pub keypair: Keypair,
}

impl Ed25519Signer {
    pub fn new(kp: Keypair) -> Self {
        Self { keypair: kp }
    }

    pub fn node_id(&self) -> NodeId {
        self.keypair.node_id()
    }
}

impl ArtifactSigner for Ed25519Signer {
    fn sign_manifest(
        &self,
        release_id: &str,
        version: Version,
        created_at_unix: u64,
        parent: Option<&str>,
        artifacts: Vec<Artifact>,
    ) -> Result<ReleaseManifest, ReleaseError> {
        let signer = self.keypair.node_id();
        let msg = ReleaseManifest::canonical_bytes(
            release_id,
            version,
            created_at_unix,
            parent,
            &artifacts,
            &signer,
        );
        let sig = self.keypair.sign(&msg);
        Ok(ReleaseManifest {
            release_id: release_id.into(),
            version,
            created_at_unix,
            parent_release_id: parent.map(|s| s.to_string()),
            artifacts,
            signer,
            signature: sig,
        })
    }

    fn verify_manifest(&self, m: &ReleaseManifest) -> Result<(), ReleaseError> {
        let msg = ReleaseManifest::canonical_bytes(
            &m.release_id,
            m.version,
            m.created_at_unix,
            m.parent_release_id.as_deref(),
            &m.artifacts,
            &m.signer,
        );
        Keypair::verify(m.signer.as_bytes(), &msg, &m.signature)
            .map_err(|_| ReleaseError::BadSignature)
    }
}

// ============================================================================
// ChecksumVerifier trait
// ============================================================================

pub trait ChecksumVerifier: Send + Sync {
    /// Verify that `bytes` hashes to the artifact's expected CID.
    fn verify(&self, artifact: &Artifact, bytes: &[u8]) -> Result<(), ReleaseError>;
}

/// Reference impl using content333::Cid (SHA-256).
pub struct ContentAddressedVerifier;

impl ChecksumVerifier for ContentAddressedVerifier {
    fn verify(&self, artifact: &Artifact, bytes: &[u8]) -> Result<(), ReleaseError> {
        let got = Cid::of(bytes);
        if got != artifact.cid {
            return Err(ReleaseError::ChecksumMismatch {
                name: artifact.name.clone(),
                expected: artifact.cid,
                got,
            });
        }
        if bytes.len() as u64 != artifact.size_bytes {
            return Err(ReleaseError::ChecksumMismatch {
                name: artifact.name.clone(),
                expected: artifact.cid,
                got: artifact.cid,
            });
        }
        Ok(())
    }
}

// ============================================================================
// ProvenanceChain
// ============================================================================

pub struct ProvenanceChain {
    releases: Mutex<BTreeMap<String, ReleaseManifest>>,
    /// Allowed signer set; manifests signed by unknown signers are rejected.
    trusted_signers: Mutex<Vec<NodeId>>,
}

impl Default for ProvenanceChain {
    fn default() -> Self {
        Self {
            releases: Mutex::new(BTreeMap::new()),
            trusted_signers: Mutex::new(Vec::new()),
        }
    }
}

impl ProvenanceChain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn trust(&self, signer: NodeId) {
        self.trusted_signers.lock().unwrap().push(signer);
    }

    pub fn is_trusted(&self, signer: &NodeId) -> bool {
        self.trusted_signers.lock().unwrap().iter().any(|s| s == signer)
    }

    pub fn append(&self, m: ReleaseManifest) -> Result<(), ReleaseError> {
        if !self.is_trusted(&m.signer) {
            return Err(ReleaseError::UnknownSigner(m.signer.clone()));
        }
        // Verify signature.
        let verifier = Ed25519Signer {
            keypair: Keypair::generate(), // placeholder; we verify via signer bytes directly.
        };
        // Use a plain verify call against the embedded signer's public key.
        // (Signer field of Ed25519Signer is ignored because verify_manifest uses the manifest's own signer.)
        verifier.verify_manifest(&m)?;

        let mut g = self.releases.lock().unwrap();
        if g.contains_key(&m.release_id) {
            return Err(ReleaseError::DuplicateRelease(m.release_id.clone()));
        }
        if let Some(parent_id) = &m.parent_release_id {
            let parent = g
                .get(parent_id)
                .ok_or_else(|| ReleaseError::UnknownParent(parent_id.clone()))?;
            if m.version <= parent.version {
                return Err(ReleaseError::SemverRegression {
                    child: m.version,
                    parent: parent.version,
                });
            }
        }
        g.insert(m.release_id.clone(), m);
        Ok(())
    }

    pub fn get(&self, release_id: &str) -> Option<ReleaseManifest> {
        self.releases.lock().unwrap().get(release_id).cloned()
    }

    pub fn latest(&self) -> Option<ReleaseManifest> {
        self.releases
            .lock()
            .unwrap()
            .values()
            .max_by_key(|m| m.version)
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.releases.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the chain from the root to `release_id`, following
    /// parent_release_id links. Returns empty vec if the release is missing.
    pub fn ancestry(&self, release_id: &str) -> Vec<ReleaseManifest> {
        let g = self.releases.lock().unwrap();
        let mut out = Vec::new();
        let mut cursor = g.get(release_id).cloned();
        while let Some(m) = cursor {
            let parent_id = m.parent_release_id.clone();
            out.push(m);
            cursor = parent_id.and_then(|pid| g.get(&pid).cloned());
        }
        out.reverse();
        out
    }
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn art(name: &str, bytes: &[u8]) -> Artifact {
        Artifact {
            name: name.into(),
            cid: Cid::of(bytes),
            size_bytes: bytes.len() as u64,
        }
    }

    // ---- Version ----

    #[test]
    fn version_parse_roundtrip() {
        let v = Version::parse("v1.2.3").unwrap();
        assert_eq!(v, Version::new(1, 2, 3));
        assert_eq!(v.to_string(), "v1.2.3");
        // Also parses without the leading v.
        assert_eq!(Version::parse("4.5.6").unwrap(), Version::new(4, 5, 6));
    }

    #[test]
    fn version_bad_input_rejected() {
        assert!(Version::parse("1.2").is_err());
        assert!(Version::parse("x.y.z").is_err());
    }

    #[test]
    fn version_ordering_is_lexicographic() {
        assert!(Version::new(1, 0, 0) < Version::new(1, 0, 1));
        assert!(Version::new(1, 0, 1) < Version::new(1, 1, 0));
        assert!(Version::new(1, 1, 9) < Version::new(2, 0, 0));
    }

    // ---- Manifest signing ----

    #[test]
    fn sign_and_verify_roundtrip() {
        let signer = Ed25519Signer::new(Keypair::generate());
        let artifacts = vec![art("sdk.wasm", b"\x00\x01\x02\x03"), art("cli.tar.gz", b"hello")];
        let m = signer
            .sign_manifest("rel-1", Version::new(1, 0, 0), 1_700_000_000, None, artifacts)
            .unwrap();
        signer.verify_manifest(&m).unwrap();
    }

    #[test]
    fn tampered_manifest_fails_verify() {
        let signer = Ed25519Signer::new(Keypair::generate());
        let mut m = signer
            .sign_manifest(
                "rel-2",
                Version::new(1, 0, 0),
                1_700_000_000,
                None,
                vec![art("x", b"x")],
            )
            .unwrap();
        m.artifacts.push(art("evil", b"evil"));
        assert!(matches!(signer.verify_manifest(&m), Err(ReleaseError::BadSignature)));
    }

    #[test]
    fn manifest_canonical_bytes_stable_across_artifact_order() {
        let signer = Ed25519Signer::new(Keypair::generate());
        let a1 = art("alpha", b"a");
        let a2 = art("bravo", b"b");
        let m1 = signer
            .sign_manifest(
                "rel-A",
                Version::new(1, 0, 0),
                1,
                None,
                vec![a1.clone(), a2.clone()],
            )
            .unwrap();
        let m2 = signer
            .sign_manifest("rel-A", Version::new(1, 0, 0), 1, None, vec![a2, a1])
            .unwrap();
        // Signatures differ (different RNG state inside sign) but canonical bytes match,
        // so the manifest content is verifiable irrespective of input order.
        let b1 = ReleaseManifest::canonical_bytes(
            &m1.release_id,
            m1.version,
            m1.created_at_unix,
            m1.parent_release_id.as_deref(),
            &m1.artifacts,
            &m1.signer,
        );
        let b2 = ReleaseManifest::canonical_bytes(
            &m2.release_id,
            m2.version,
            m2.created_at_unix,
            m2.parent_release_id.as_deref(),
            &m2.artifacts,
            &m2.signer,
        );
        assert_eq!(b1, b2);
    }

    // ---- Checksum verifier ----

    #[test]
    fn checksum_verifier_accepts_correct_bytes() {
        let bytes = b"payload";
        let a = art("x", bytes);
        ContentAddressedVerifier.verify(&a, bytes).unwrap();
    }

    #[test]
    fn checksum_verifier_rejects_tampered_bytes() {
        let a = art("x", b"original");
        let err = ContentAddressedVerifier.verify(&a, b"tampered").unwrap_err();
        assert!(matches!(err, ReleaseError::ChecksumMismatch { .. }));
    }

    #[test]
    fn checksum_verifier_rejects_wrong_size() {
        // Same bytes but a spoofed size_bytes field.
        let mut a = art("x", b"abcd");
        a.size_bytes = 999;
        let err = ContentAddressedVerifier.verify(&a, b"abcd").unwrap_err();
        assert!(matches!(err, ReleaseError::ChecksumMismatch { .. }));
    }

    // ---- ProvenanceChain ----

    fn signed_manifest(
        signer: &Ed25519Signer,
        rid: &str,
        v: Version,
        parent: Option<&str>,
    ) -> ReleaseManifest {
        signer
            .sign_manifest(rid, v, 1_700_000_000, parent, vec![art("sdk.wasm", b"abc")])
            .unwrap()
    }

    #[test]
    fn chain_accepts_signed_root_release() {
        let signer = Ed25519Signer::new(Keypair::generate());
        let chain = ProvenanceChain::new();
        chain.trust(signer.node_id());
        let m = signed_manifest(&signer, "rel-0", Version::new(1, 0, 0), None);
        chain.append(m).unwrap();
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn chain_rejects_untrusted_signer() {
        let signer = Ed25519Signer::new(Keypair::generate());
        let chain = ProvenanceChain::new();
        // DO NOT trust signer.
        let m = signed_manifest(&signer, "rel-0", Version::new(1, 0, 0), None);
        let err = chain.append(m).unwrap_err();
        assert!(matches!(err, ReleaseError::UnknownSigner(_)));
    }

    #[test]
    fn chain_rejects_semver_regression() {
        let signer = Ed25519Signer::new(Keypair::generate());
        let chain = ProvenanceChain::new();
        chain.trust(signer.node_id());
        chain
            .append(signed_manifest(&signer, "rel-0", Version::new(2, 0, 0), None))
            .unwrap();
        let child = signed_manifest(&signer, "rel-1", Version::new(1, 9, 9), Some("rel-0"));
        let err = chain.append(child).unwrap_err();
        assert!(matches!(err, ReleaseError::SemverRegression { .. }));
    }

    #[test]
    fn chain_rejects_unknown_parent() {
        let signer = Ed25519Signer::new(Keypair::generate());
        let chain = ProvenanceChain::new();
        chain.trust(signer.node_id());
        let m = signed_manifest(&signer, "rel-1", Version::new(1, 0, 0), Some("rel-0"));
        let err = chain.append(m).unwrap_err();
        assert!(matches!(err, ReleaseError::UnknownParent(_)));
    }

    #[test]
    fn chain_rejects_duplicate_release_id() {
        let signer = Ed25519Signer::new(Keypair::generate());
        let chain = ProvenanceChain::new();
        chain.trust(signer.node_id());
        chain
            .append(signed_manifest(&signer, "rel-0", Version::new(1, 0, 0), None))
            .unwrap();
        let dup = signed_manifest(&signer, "rel-0", Version::new(1, 0, 1), None);
        let err = chain.append(dup).unwrap_err();
        assert!(matches!(err, ReleaseError::DuplicateRelease(_)));
    }

    #[test]
    fn chain_latest_picks_max_version() {
        let signer = Ed25519Signer::new(Keypair::generate());
        let chain = ProvenanceChain::new();
        chain.trust(signer.node_id());
        chain
            .append(signed_manifest(&signer, "rel-0", Version::new(1, 0, 0), None))
            .unwrap();
        chain
            .append(signed_manifest(&signer, "rel-1", Version::new(1, 1, 0), Some("rel-0")))
            .unwrap();
        chain
            .append(signed_manifest(&signer, "rel-2", Version::new(2, 0, 0), Some("rel-1")))
            .unwrap();
        let latest = chain.latest().unwrap();
        assert_eq!(latest.release_id, "rel-2");
    }

    #[test]
    fn chain_ancestry_returns_root_to_child_order() {
        let signer = Ed25519Signer::new(Keypair::generate());
        let chain = ProvenanceChain::new();
        chain.trust(signer.node_id());
        chain
            .append(signed_manifest(&signer, "a", Version::new(1, 0, 0), None))
            .unwrap();
        chain
            .append(signed_manifest(&signer, "b", Version::new(1, 1, 0), Some("a")))
            .unwrap();
        chain
            .append(signed_manifest(&signer, "c", Version::new(1, 2, 0), Some("b")))
            .unwrap();
        let line = chain.ancestry("c");
        let ids: Vec<&str> = line.iter().map(|m| m.release_id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn chain_rejects_tampered_manifest() {
        let signer = Ed25519Signer::new(Keypair::generate());
        let chain = ProvenanceChain::new();
        chain.trust(signer.node_id());
        let mut m = signed_manifest(&signer, "rel-0", Version::new(1, 0, 0), None);
        // Tamper after signing.
        m.artifacts.push(art("extra", b"x"));
        assert!(matches!(chain.append(m), Err(ReleaseError::BadSignature)));
    }

    #[test]
    fn chain_is_empty_initially() {
        let chain = ProvenanceChain::new();
        assert!(chain.is_empty());
        assert!(chain.latest().is_none());
    }

    #[test]
    fn chain_forks_allowed_same_parent() {
        // Branch releases: two children off the same parent with different versions.
        let signer = Ed25519Signer::new(Keypair::generate());
        let chain = ProvenanceChain::new();
        chain.trust(signer.node_id());
        chain
            .append(signed_manifest(&signer, "root", Version::new(1, 0, 0), None))
            .unwrap();
        chain
            .append(signed_manifest(&signer, "branch-a", Version::new(1, 1, 0), Some("root")))
            .unwrap();
        chain
            .append(signed_manifest(&signer, "branch-b", Version::new(1, 2, 0), Some("root")))
            .unwrap();
        assert_eq!(chain.len(), 3);
        // Latest is branch-b (highest version).
        assert_eq!(chain.latest().unwrap().release_id, "branch-b");
    }
}
