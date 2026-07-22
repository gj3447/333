# Third-party provenance and notices

This document records source-code provenance and research lineage for 333. It
does not replace the license texts of 333 or any upstream project. The 333
repository as a combined work is distributed under `AGPL-3.0-only`; see
[`LICENSE`](LICENSE).

## Garage CRDT code — modified source

Parts of [`333-crdt`](333-crdt/) are modified Rust adaptations of the
`garage_util::crdt` module from Garage:

- Upstream project: [Garage](https://git.deuxfleurs.fr/Deuxfleurs/garage)
  ([GitHub mirror](https://github.com/deuxfleurs-org/garage))
- Upstream source: [`src/util/crdt/`](https://github.com/deuxfleurs-org/garage/tree/main-v2/src/util/crdt)
- License-audit reference: Garage commit
  [`663fc5ae486562f0a67d63be0071b03c822c0ed5`](https://github.com/deuxfleurs-org/garage/tree/663fc5ae486562f0a67d63be0071b03c822c0ed5/src/util/crdt).
  The repository did not record the original port base, so this is an audit
  reference rather than a claim that it was the exact source revision used.
- Upstream license: GNU Affero General Public License version 3
- Upstream copyright: Garage contributors

The adapted scope includes the `Crdt`/`AutoCrdt` traits, `Lww`, `LwwMap`,
`Map`, `Bool`, and `Deletable` types. The corresponding 333 files are
`333-crdt/src/traits.rs`, `lww.rs`, `map.rs`, and `scalar.rs`, with exports in
`lib.rs`.

333 modifications were made in 2026. They include a reduced standalone API,
explicit timestamp injection, dependency and logging removal, altered trait
bounds and primitive implementations, deterministic merge code, tests, and
additional CRDT modules. The prior claim that this was a clean-room
implementation under Apache-2.0/MIT has been withdrawn. The adapted code and
333 modifications are now distributed under `AGPL-3.0-only`.

## Puter kernel patterns — Rust/WASM adaptations

The files below implement Rust/WASM adaptations based on the repository's
2026 TPA design analysis of Puter:

- Upstream project: [Puter](https://github.com/HeyPuter/puter)
- Upstream license: `AGPL-3.0-only`
- Upstream copyright: Puter contributors
- 333 scope: `333-platform/src/kernel/{mod,capability,channel,lifecycle,manifest,service,worker}.rs`

The relationship is architectural and pattern-level: service registration,
manifest/version negotiation, actor/capability context, service lifecycle,
event channels, and worker scheduling were translated into 333-specific Rust
types and state machines in 2026. No Puter source file is vendored verbatim in
this repository. These notices preserve the existing `ported`, `mirrors`, and
`inspired by` provenance labels instead of recasting the work as clean-room.
The adaptations are distributed under the repository's
`AGPL-3.0-only` license.

## Research and design references — no bundled source claimed

The repository also names the following projects as API, architecture, or
design references. Current 333 source comments describe these relationships as
`inspired by`, `pattern`, or `design invariants`; this notice does **not** claim
that their source code is bundled or that the listed 333 modules are direct
derivatives:

- [Kubo](https://github.com/ipfs/kubo): identity, content, messaging, and plugin
  API shapes.
- [SeaweedFS](https://github.com/seaweedfs/seaweedfs): topology, IAM, storage,
  and erasure-profile design references.
- [wasmCloud](https://github.com/wasmCloud/wasmCloud): workload lifecycle and
  host-control patterns in the platform/hypervisor line.
- [etcd](https://github.com/etcd-io/etcd): WAL durability invariants documented
  in `substrate/crates/wal`.

Those upstream projects retain their own copyright and license terms. Rust,
JavaScript, and other package dependencies likewise remain governed by the
licenses supplied by their respective authors and package distributions.

## Academic lineage

FastPay, Zef, Sui Lutris/Mysticeti, Linera, and the CRDT literature are cited as
research lineage in repository documentation. Those citations identify ideas
and papers; they are not declarations that paper source code is included.
