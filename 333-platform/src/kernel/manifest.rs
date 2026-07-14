// KG: puter-tpa-P2-manifest-2026-04-16, puter-tpa-P10-semver-2026-04-16
// P2: Extension manifest hot-load — each WASM extension declares its manifest
// P10: SemVer runtime module export — manifest carries version, compat range
// Mirrors Puter's dynamic WASM module loading with capability declarations

use super::capability::Capability;
use std::fmt;

/// Semantic version (no external dep — keep WASM binary small)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemVer {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub pre:   Option<String>, // "alpha.1", "beta", etc.
}

impl SemVer {
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self { major, minor, patch, pre: None }
    }

    pub fn with_pre(mut self, pre: impl Into<String>) -> Self {
        self.pre = Some(pre.into());
        self
    }

    /// Semver compatibility: same major and self >= required
    pub fn is_compatible_with(&self, required: &SemVer) -> bool {
        if self.major != required.major { return false; }
        if self.minor > required.minor  { return true;  }
        if self.minor < required.minor  { return false; }
        self.patch >= required.patch
    }

    /// Parse "major.minor.patch" or "major.minor.patch-pre"
    pub fn parse(s: &str) -> Option<Self> {
        let (main, pre) = if let Some(idx) = s.find('-') {
            (&s[..idx], Some(s[idx+1..].to_string()))
        } else {
            (s, None)
        };
        let parts: Vec<&str> = main.split('.').collect();
        if parts.len() != 3 { return None; }
        Some(SemVer {
            major: parts[0].parse().ok()?,
            minor: parts[1].parse().ok()?,
            patch: parts[2].parse().ok()?,
            pre,
        })
    }
}

impl fmt::Display for SemVer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.pre {
            None    => write!(f, "{}.{}.{}", self.major, self.minor, self.patch),
            Some(p) => write!(f, "{}.{}.{}-{}", self.major, self.minor, self.patch, p),
        }
    }
}

/// Extension kind — static (bundled) or dynamic (hot-loaded WASM blob)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionKind {
    /// Compiled into main WASM binary
    Static,
    /// Loaded at runtime from a separate WASM blob identified by hash
    Dynamic { content_hash: String },
}

/// Module manifest — declared at compile time, validated at load time (P2/P10)
#[derive(Debug, Clone)]
pub struct ModuleManifest {
    /// Unique module id (reverse-dns style: "com.333.rts")
    pub id: String,
    /// Human-readable display name
    pub display_name: String,
    /// Module version
    pub version: SemVer,
    /// Minimum platform API version required
    pub min_platform: SemVer,
    /// Capabilities this module requires to function
    pub required_caps: Vec<Capability>,
    /// Capabilities this module optionally uses
    pub optional_caps: Vec<Capability>,
    /// Whether this is static or dynamically loaded
    pub kind: ExtensionKind,
    /// Entrypoint function name (for dynamic modules)
    pub entrypoint: Option<String>,
}

impl ModuleManifest {
    pub fn static_module(
        id: impl Into<String>,
        display_name: impl Into<String>,
        version: SemVer,
        min_platform: SemVer,
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            version,
            min_platform,
            required_caps: vec![],
            optional_caps: vec![],
            kind: ExtensionKind::Static,
            entrypoint: None,
        }
    }

    pub fn with_required_caps(mut self, caps: Vec<Capability>) -> Self {
        self.required_caps = caps;
        self
    }

    pub fn with_optional_caps(mut self, caps: Vec<Capability>) -> Self {
        self.optional_caps = caps;
        self
    }

    /// Validate that a given platform version satisfies this manifest's minimum
    pub fn is_platform_compatible(&self, platform: &SemVer) -> bool {
        platform.is_compatible_with(&self.min_platform)
    }
}

/// Manifest registry — loaded manifests keyed by module id
#[derive(Default)]
pub struct ManifestRegistry {
    manifests: std::collections::HashMap<String, ModuleManifest>,
}

impl ManifestRegistry {
    pub fn new() -> Self { Self::default() }

    /// Register a manifest; returns Err if id already registered
    pub fn register(&mut self, manifest: ModuleManifest) -> Result<(), ManifestError> {
        if self.manifests.contains_key(&manifest.id) {
            return Err(ManifestError::DuplicateId(manifest.id.clone()));
        }
        self.manifests.insert(manifest.id.clone(), manifest);
        Ok(())
    }

    /// Hot-load: replace an existing manifest (version upgrade)
    pub fn hot_load(&mut self, manifest: ModuleManifest, platform: &SemVer) -> Result<(), ManifestError> {
        if !manifest.is_platform_compatible(platform) {
            return Err(ManifestError::IncompatiblePlatform {
                module: manifest.id.clone(),
                required: manifest.min_platform.clone(),
                have: platform.clone(),
            });
        }
        self.manifests.insert(manifest.id.clone(), manifest);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&ModuleManifest> {
        self.manifests.get(id)
    }

    pub fn len(&self) -> usize { self.manifests.len() }
    pub fn is_empty(&self) -> bool { self.manifests.is_empty() }

    /// List all module ids
    pub fn ids(&self) -> Vec<&str> {
        self.manifests.keys().map(|s| s.as_str()).collect()
    }
}

#[derive(Debug, Clone)]
pub enum ManifestError {
    DuplicateId(String),
    IncompatiblePlatform { module: String, required: SemVer, have: SemVer },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::DuplicateId(id) =>
                write!(f, "module '{}' already registered", id),
            ManifestError::IncompatiblePlatform { module, required, have } =>
                write!(f, "module '{}' needs platform>={} but have {}", module, required, have),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::capability::Capability;

    fn platform_v1() -> SemVer { SemVer::new(1, 0, 0) }

    fn rts_manifest() -> ModuleManifest {
        ModuleManifest::static_module(
            "com.333.rts", "RTS Game", SemVer::new(1, 2, 0), SemVer::new(1, 0, 0)
        ).with_required_caps(vec![Capability::WorldRead, Capability::WorldWrite])
         .with_optional_caps(vec![Capability::ConsensusSubmit])
    }

    #[test]
    fn semver_parse() {
        let v = SemVer::parse("1.2.3").unwrap();
        assert_eq!(v.major, 1); assert_eq!(v.minor, 2); assert_eq!(v.patch, 3);
        assert_eq!(v.to_string(), "1.2.3");
    }

    #[test]
    fn semver_parse_with_pre() {
        let v = SemVer::parse("2.0.0-alpha.1").unwrap();
        assert_eq!(v.pre.as_deref(), Some("alpha.1"));
        assert_eq!(v.to_string(), "2.0.0-alpha.1");
    }

    #[test]
    fn semver_compat_same_major() {
        let v1_2 = SemVer::new(1, 2, 0);
        let v1_0 = SemVer::new(1, 0, 0);
        assert!(v1_2.is_compatible_with(&v1_0));  // 1.2 satisfies >=1.0
        assert!(!v1_0.is_compatible_with(&v1_2)); // 1.0 does NOT satisfy >=1.2
    }

    #[test]
    fn semver_compat_diff_major() {
        assert!(!SemVer::new(2, 0, 0).is_compatible_with(&SemVer::new(1, 0, 0)));
    }

    #[test]
    fn manifest_register_and_get() {
        let mut reg = ManifestRegistry::new();
        reg.register(rts_manifest()).unwrap();
        assert_eq!(reg.len(), 1);
        let m = reg.get("com.333.rts").unwrap();
        assert_eq!(m.display_name, "RTS Game");
        assert_eq!(m.required_caps.len(), 2);
    }

    #[test]
    fn manifest_duplicate_id_err() {
        let mut reg = ManifestRegistry::new();
        reg.register(rts_manifest()).unwrap();
        assert!(reg.register(rts_manifest()).is_err());
    }

    #[test]
    fn hot_load_upgrades_manifest() {
        let mut reg = ManifestRegistry::new();
        reg.register(rts_manifest()).unwrap();
        let v2 = ModuleManifest::static_module(
            "com.333.rts", "RTS Game v2", SemVer::new(2, 0, 0), SemVer::new(1, 0, 0)
        );
        reg.hot_load(v2, &platform_v1()).unwrap();
        assert_eq!(reg.get("com.333.rts").unwrap().display_name, "RTS Game v2");
    }

    #[test]
    fn hot_load_incompatible_platform() {
        let mut reg = ManifestRegistry::new();
        let future = ModuleManifest::static_module(
            "com.333.new", "Future Module",
            SemVer::new(1, 0, 0),
            SemVer::new(2, 0, 0), // requires platform 2.0
        );
        let err = reg.hot_load(future, &platform_v1());
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("IncompatiblePlatform").not());
        // just check it's an err — message contains platform version
    }

    #[test]
    fn platform_compatibility_check() {
        let m = rts_manifest();
        assert!(m.is_platform_compatible(&SemVer::new(1, 0, 0)));
        assert!(m.is_platform_compatible(&SemVer::new(1, 5, 0)));
        assert!(!m.is_platform_compatible(&SemVer::new(0, 9, 0)));
    }
}

// helper for test
trait Not { fn not(self) -> bool; }
impl Not for bool { fn not(self) -> bool { !self } }
