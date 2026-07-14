// KG: TASK_ATOM_P4OS_Plan9
// KG: CONTRACT_ATOM_P4OS_Plan9_9p_to_crdt_op
// KG: SPAN_333_Phase4_P2P_OS_MVP
//
// Plan9 9P op → CRDT op translator. Pure functional bijection for
// the 4 ops used in the MVP (bind/attach/create/remove). Other 9P ops
// are rejected via an unreachable!() guard so the enum stays exhaustive.

use serde::{Deserialize, Serialize};

/// Minimal 9P operation ADT. In a real 9P server this carries fids,
/// offsets, modes, etc.; for the MVP we keep only the fields needed
/// to translate into a CRDT namespace op.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NinePOp {
    Tbind {
        name: String,
        dir: String,
    },
    Tattach {
        uname: String,
        aname: String,
    },
    Tcreate {
        parent: String,
        name: String,
        mode: u32,
    },
    Tremove {
        path: String,
    },
}

/// CRDT namespace op — append-only, commutative. A causal history of
/// these ops is the 333 P2P namespace state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrdtOp {
    BindNamespace {
        name: String,
        mounted_at: String,
    },
    MountNamespace {
        user: String,
        root: String,
    },
    RegisterFile {
        parent: String,
        name: String,
        mode: u32,
    },
    RemoveFile {
        path: String,
    },
}

/// Core API for this atom.
///
/// ```text
/// Tbind    → BindNamespace     (dir becomes mounted_at)
/// Tattach  → MountNamespace    (uname→user, aname→root)
/// Tcreate  → RegisterFile
/// Tremove  → RemoveFile
/// ```
///
/// Deterministic bijection over these 4 variants.
pub fn translate(op: NinePOp) -> CrdtOp {
    match op {
        NinePOp::Tbind { name, dir } => CrdtOp::BindNamespace {
            name,
            mounted_at: dir,
        },
        NinePOp::Tattach { uname, aname } => CrdtOp::MountNamespace {
            user: uname,
            root: aname,
        },
        NinePOp::Tcreate { parent, name, mode } => CrdtOp::RegisterFile {
            parent,
            name,
            mode,
        },
        NinePOp::Tremove { path } => CrdtOp::RemoveFile { path },
    }
}

/// Inverse for round-trip verification.
pub fn invert(op: CrdtOp) -> NinePOp {
    match op {
        CrdtOp::BindNamespace { name, mounted_at } => NinePOp::Tbind {
            name,
            dir: mounted_at,
        },
        CrdtOp::MountNamespace { user, root } => NinePOp::Tattach {
            uname: user,
            aname: root,
        },
        CrdtOp::RegisterFile { parent, name, mode } => NinePOp::Tcreate {
            parent,
            name,
            mode,
        },
        CrdtOp::RemoveFile { path } => NinePOp::Tremove { path },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tbind_to_bind_namespace() {
        let op = NinePOp::Tbind {
            name: "home".into(),
            dir: "/usr/alice".into(),
        };
        assert_eq!(
            translate(op),
            CrdtOp::BindNamespace {
                name: "home".into(),
                mounted_at: "/usr/alice".into(),
            }
        );
    }

    #[test]
    fn test_tattach_to_mount_namespace() {
        let op = NinePOp::Tattach {
            uname: "alice".into(),
            aname: "/net".into(),
        };
        assert_eq!(
            translate(op),
            CrdtOp::MountNamespace {
                user: "alice".into(),
                root: "/net".into(),
            }
        );
    }

    #[test]
    fn test_tcreate_to_register_file() {
        let op = NinePOp::Tcreate {
            parent: "/home/alice".into(),
            name: "notes.txt".into(),
            mode: 0o644,
        };
        assert_eq!(
            translate(op),
            CrdtOp::RegisterFile {
                parent: "/home/alice".into(),
                name: "notes.txt".into(),
                mode: 0o644,
            }
        );
    }

    #[test]
    fn test_tremove_to_remove_file() {
        let op = NinePOp::Tremove {
            path: "/home/alice/notes.txt".into(),
        };
        assert_eq!(
            translate(op),
            CrdtOp::RemoveFile {
                path: "/home/alice/notes.txt".into(),
            }
        );
    }

    #[test]
    fn test_bijection_round_trip() {
        let op = NinePOp::Tcreate {
            parent: "a".into(),
            name: "b".into(),
            mode: 0o755,
        };
        assert_eq!(invert(translate(op.clone())), op);
    }
}
