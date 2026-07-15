// Idempotent KG registration for PROM 4 / 333 payment safety.
// Execute against the canonical DGX Neo4j database.

MERGE (tree:LakatosTree {name: 'LakatosTree_333PaymentSafety_20260715'})
SET tree.title = '333/ORRR Payment Safety — authenticated FastPay + BFT shared state',
    tree.hard_core = 'One conserved unit of value: public-key-bound single-owner debits use per-owner order; multi-writer marketplace transitions use Byzantine total order; no unsigned or caller-asserted proof mutates either lane.',
    tree.frontier_rule = 'falsifier-first; n-f quorum; persist-before-sign; one-shot proof bridge',
    tree.scope = 'logical_cs_distributed_systems_only',
    tree.status = 'PROGRESSIVE_CORE',
    tree.sourcePath = '333-payment/PROM_4_PAYMENT_SAFETY_2026-07-15.md',
    tree.minioPath = 's3://docs/333/payment-safety/2026-07-15/PROM_4_PAYMENT_SAFETY_2026-07-15.md',
    tree.source_commit = 'c4c10e192f6334e30cb007dc56883a7ec4e09554',
    tree.createdAt = coalesce(tree.createdAt, '2026-07-15T00:00:00+09:00'),
    tree.updatedAt = '2026-07-15T00:00:00+09:00';

MERGE (core:LakatosHardCore {name: 'lakatos-hard-core-333-payment-safety-2026-07-15'})
SET core.status = 'CANONICAL_FOR_THIS_PROGRAMME',
    core.hard_core_principles = [
      'P1: value has exactly one conservation invariant',
      'P2: only the public-key-bound owner may debit an ordinary account',
      'P3: independent owner debits need per-owner order, not global total order',
      'P4: escrow, bids, disputes, rewards and committee changes are multi-writer and require Byzantine total order',
      'P5: every safety decision is durable before its signature is released',
      'P6: signed data identifies protocol, network, asset, genesis and committee epoch/roster'
    ],
    core.negative_heuristic = 'Do not restore unsigned owner orders, memory-only signing locks, context-free signatures, or a second writable token ledger.',
    core.progressive_criterion = 'A belt move closes a concrete counterexample without weakening P1-P6 and yields a passing adversarial falsifier.',
    core.sourcePath = '333-payment/PROM_4_PAYMENT_SAFETY_2026-07-15.md';

MERGE (cycle:PrometheusCycle:ResearchCycle {name: 'prom4-333-payment-safety-2026-07-15'})
SET cycle.cycle_id = 'prom4-333-payment-safety-2026-07-15',
    cycle.status = 'COMPLETE',
    cycle.scope = 'logical_cs_distributed_systems_only',
    cycle.trigger = 'user-direct-speech: solve defects 1,2,3,4; run PROM; place research on LakatosTree',
    cycle.axes = ['owner-authentication', 'durable-safety-state', 'domain-and-epoch-binding', 'single-economic-ledger-two-proof-typed-lanes'],
    cycle.sourcePath = '333-payment/PROM_4_PAYMENT_SAFETY_2026-07-15.md',
    cycle.source_commit = 'c4c10e192f6334e30cb007dc56883a7ec4e09554',
    cycle.verifiedAt = '2026-07-15T00:00:00+09:00';

MERGE (lesson:DefectCluster:AbstractNode {name: 'lesson-333-payment-four-coupled-safety-defects-2026-07-15'})
SET lesson.cycle_id = 'prom4-333-payment-safety-2026-07-15',
    lesson.severity = 'CRITICAL',
    lesson.problem = 'Unsigned string owner; restart-erased authority locks; signatures without network/asset/genesis/committee epoch; independently writable transfer333 and token333 monetary rails.',
    lesson.wrongAssumption = 'An authority quorum over an unsigned transfer plus an in-memory sequence map is sufficient, and FastPay/token ledgers can be composed later without an explicit bridge.',
    lesson.truth = 'FastPay authenticates the owner and persists per-account safety state. Multi-writer ORRR state needs ordered BFT control, while routine payouts retain the per-owner fast path. The lanes may cross only through proof-typed, idempotent settlement effects.',
    lesson.solution = 'payment333: owner-signed context-bound orders; fsync-before-vote safety files; epoch/roster rotation drain; certified escrow deposits; BFT job/reward state; one-shot reserve-backed vouchers; one total-supply invariant.',
    lesson.resolved = false,
    lesson.resolution_pending_link = true,
    lesson.sourcePath = '333-payment/tests/payment_safety.rs';

MERGE (artifact:ImplementationArtifact:VerifiedArtifact {name: 'payment333-prom4-implementation-2026-07-15'})
SET artifact.status = 'VERIFIED',
    artifact.branch = 'codex/prom-333-safety',
    artifact.commit = 'c4c10e192f6334e30cb007dc56883a7ec4e09554',
    artifact.sourcePath = '333-payment/',
    artifact.minioPath = 's3://docs/333/payment-safety/2026-07-15/',
    artifact.test_command = 'cargo test --release --manifest-path /Users/lagyeongjun/CD/worktrees/333-prom-safety/333-payment/Cargo.toml',
    artifact.test_result = '7 passed; 0 failed',
    artifact.regression_result = 'transfer333 65 passed (1 ignored); token333 17 passed; incentive333 16 passed',
    artifact.verifiedAt = '2026-07-15T00:00:00+09:00';

MERGE (appraisal:LakatosAppraisal:AbstractNode {name: 'lakatos-333-payment-safety-2026-07-15'})
SET appraisal.cycle_id = 'prom4-333-payment-safety-2026-07-15',
    appraisal.verdict = 'PROGRESSIVE_CORE',
    appraisal.status = 'CANONICAL_FOR_IMPLEMENTED_CORE',
    appraisal.rationale = 'All four reviewed defects have executable countermeasures and direct falsifiers. Excess content: split BFT commitment avoids reintroducing global ordering; applied-funding binding prevents cross-job escrow consumption; rotation drains pending signatures without stranding settled deposits.',
    appraisal.protective_belt_move = 'OwnerSignedOrder + DurableSafety + PaymentContext + RotationDrain + CertifiedDepositVoucherBridge + SplitCommitment',
    appraisal.degenerating_risk = 'Control safety/finality boundary is implemented; production pacemaker/view-change/network liveness remains a release gate. A Byzantine proposer may halt a height but cannot commit two values.',
    appraisal.sourcePath = '333-payment/PROM_4_PAYMENT_SAFETY_2026-07-15.md',
    appraisal.createdAt = '2026-07-15T00:00:00+09:00';

MATCH (tree:LakatosTree {name: 'LakatosTree_333PaymentSafety_20260715'}),
      (core:LakatosHardCore {name: 'lakatos-hard-core-333-payment-safety-2026-07-15'}),
      (cycle:PrometheusCycle {name: 'prom4-333-payment-safety-2026-07-15'}),
      (lesson:DefectCluster {name: 'lesson-333-payment-four-coupled-safety-defects-2026-07-15'}),
      (artifact:ImplementationArtifact {name: 'payment333-prom4-implementation-2026-07-15'}),
      (appraisal:LakatosAppraisal {name: 'lakatos-333-payment-safety-2026-07-15'})
MERGE (tree)-[:HAS_HARD_CORE]->(core)
MERGE (tree)-[:HAS_CYCLE]->(cycle)
MERGE (tree)-[:HAS_APPRAISAL]->(appraisal)
MERGE (cycle)-[:RESOLVES]->(lesson)
MERGE (cycle)-[:IMPLEMENTED_BY]->(artifact)
MERGE (artifact)-[:RESOLVES]->(lesson)
MERGE (appraisal)-[:EVALUATES]->(cycle)
SET lesson.resolved = true,
    lesson.resolution_pending_link = false,
    lesson.resolvedBy = 'payment333@c4c10e192f6334e30cb007dc56883a7ec4e09554',
    lesson.resolvedAt = '2026-07-15T00:00:00+09:00';

MATCH (tree:LakatosTree {name: 'LakatosTree_333PaymentSafety_20260715'}),
      (cycle:PrometheusCycle {name: 'prom4-333-payment-safety-2026-07-15'}),
      (artifact:ImplementationArtifact {name: 'payment333-prom4-implementation-2026-07-15'})
UNWIND [
  {name:'rf-333pay-A-owner-authentication', domain:'owner-authentication', citation:'https://sonnino.com/papers/fastpay.pdf', summary:'FastPay hashes the owner verification key into the account address; the sender signs address, recipient, amount and sequence; authorities verify that signature, positive amount, exact sequence and sufficient balance.'},
  {name:'rf-333pay-B-crash-safety', domain:'durable-safety-state', citation:'https://pkg.go.dev/github.com/tendermint/tendermint@v0.35.9/internal/consensus', summary:'Safety decisions require durable write-and-sync/replay semantics before signing; payment333 persists account/control locks before returning Ed25519 votes.'},
  {name:'rf-333pay-C-domain-replay', domain:'domain-and-epoch-binding', citation:'https://docs.cosmos.network/sdk/latest/learn/concepts/encoding', summary:'Chain/domain id and account sequence belong in deterministic sign bytes. payment333 extends this to protocol/network/asset/genesis/committee epoch and roster-derived id.'},
  {name:'rf-333pay-D-marketplace-shared-state', domain:'proof-typed-lane-integration', citation:'https://akash.network/docs/node-operators/architecture/application-layer/', summary:'Compute orders, bids, leases, escrow, payout and refund are shared lifecycle state. payment333 BFT-orders that state while leaving routine single-owner payouts on the FastPay lane.'}
] AS row
MERGE (finding:ResearchFinding:LakatosExternalEvidence {name: row.name})
SET finding.cycle_id = cycle.name,
    finding.domain = row.domain,
    finding.citation_url = row.citation,
    finding.oneLineSummary = row.summary,
    finding.confidence = 'HIGH',
    finding.verified = true,
    finding.researchedAt = '2026-07-15T00:00:00+09:00',
    finding.sourcePath = '333-payment/PROM_4_PAYMENT_SAFETY_2026-07-15.md'
MERGE (cycle)-[:HAS_RESEARCH]->(finding)
MERGE (artifact)-[:GROUNDED_IN]->(finding)
MERGE (tree)-[:HAS_EXTERNAL_EVIDENCE]->(finding);

MATCH (tree:LakatosTree {name: 'LakatosTree_333PaymentSafety_20260715'}),
      (cycle:PrometheusCycle {name: 'prom4-333-payment-safety-2026-07-15'})
UNWIND [
  {name:'prediction-333pay-P1-owner-auth', claim:'Tampered amount or mismatched account/public key never obtains an honest authority vote.', falsifier:'owner_signature_and_pubkey_bound_account_are_mandatory fails', evidence:'333-payment/tests/payment_safety.rs::owner_signature_and_pubkey_bound_account_are_mandatory'},
  {name:'prediction-333pay-P2-restart', claim:'Restarting the same authority key/state cannot erase a transfer or control signing lock.', falsifier:'restart_preserves_transfer_and_control_signing_locks fails', evidence:'333-payment/tests/payment_safety.rs::restart_preserves_transfer_and_control_signing_locks'},
  {name:'prediction-333pay-P3-context', claim:'Another network or retired committee epoch certificate cannot mutate the current fast ledger.', falsifier:'network_asset_genesis_and_committee_epoch_are_signature_bound fails', evidence:'333-payment/tests/payment_safety.rs::network_asset_genesis_and_committee_epoch_are_signature_bound'},
  {name:'prediction-333pay-P4-rail-idempotency', claim:'Zero-vote control, reused funding, reused reward epoch or redeemed voucher cannot mutate state twice.', falsifier:'unified_fast_and_bft_lanes_preserve_supply_and_are_idempotent fails', evidence:'333-payment/tests/payment_safety.rs::unified_fast_and_bft_lanes_preserve_supply_and_are_idempotent'},
  {name:'prediction-333pay-P5-conservation', claim:'Deposit, dispute, payout, refund, reward and routine-transfer interleavings preserve total supply.', falsifier:'total_supply differs from genesis supply in any adversarial test', evidence:'333-payment/tests/payment_safety.rs'},
  {name:'prediction-333pay-P6-rotation-drain', claim:'Rotation rejects unsettled locks but settled historical escrow funding remains consumable.', falsifier:'rotation_cannot_cross_an_unsettled_fastpay_lock or settled_escrow_deposit_survives_committee_rotation fails', evidence:'333-payment/tests/payment_safety.rs'}
] AS row
MERGE (prediction:LakatosPrediction:AbstractNode {name: row.name})
SET prediction.claim = row.claim,
    prediction.falsifier = row.falsifier,
    prediction.evidence_test = row.evidence,
    prediction.status = 'CONFIRMED',
    prediction.progressive_or_degenerating = 'PROGRESSIVE',
    prediction.cycle_id = cycle.name,
    prediction.createdAt = '2026-07-15T00:00:00+09:00'
MERGE (tree)-[:HAS_PREDICTION]->(prediction)
MERGE (cycle)-[:CONFIRMED]->(prediction);

MATCH (tree:LakatosTree {name: 'LakatosTree_333PaymentSafety_20260715'})
OPTIONAL MATCH (anchor {name: '333_Platform'})
FOREACH (_ IN CASE WHEN anchor IS NULL THEN [] ELSE [1] END |
  MERGE (tree)-[:REFINES]->(anchor)
);

MATCH (tree:LakatosTree {name: 'LakatosTree_333PaymentSafety_20260715'})
RETURN tree.name AS tree, tree.status AS status, tree.source_commit AS commit;

MATCH (tree:LakatosTree {name: 'LakatosTree_333PaymentSafety_20260715'}),
      (cycle:PrometheusCycle {name: 'prom4-333-payment-safety-2026-07-15'}),
      (method:Methodology {name: 'OOPTDD_methodology_v1'})
MERGE (run:OOPTDDRun:VerifiedArtifact {name: 'ooptdd-payment333-prom4-20260715'})
SET run.status = 'COMPLETE',
    run.cid = 'payment333-prom4-ooptdd-20260715',
    run.requirements_done = 7,
    run.requirements_total = 7,
    run.methodology_checks_passed = 14,
    run.longinus_bindings_bound = 7,
    run.runtime_revision = '11c2e1a9c42b29d3440b1c21e650a4021c2d590d',
    run.execution_revision = '0106098f64465066eefd0d1e82be3a8990377672',
    run.execution_host = 'lagyeongjun@bhgman.iptime.org',
    run.sourcePath = '333-payment/OOPTDD_RECEIPT_PROM4_PAYMENT_2026-07-15.json',
    run.minioPath = 's3://docs/333/payment-safety/2026-07-15/OOPTDD_RECEIPT_PROM4_PAYMENT_2026-07-15.json',
    run.receipt_sha256 = '13f653f3d77ee6aacfcb89913d7efd34097744ea52b095b87dbbdba19d69b256',
    run.verifiedAt = '2026-07-15T12:45:33+09:00'
MERGE (tree)-[:APPLIES_METHODOLOGY]->(method)
MERGE (tree)-[:HAS_OOPTDD_RUN]->(run)
MERGE (cycle)-[:VERIFIED_BY]->(run)
MERGE (run)-[:USES_METHODOLOGY]->(method);

MATCH (tree:LakatosTree {name: 'LakatosTree_333PaymentSafety_20260715'}),
      (run:OOPTDDRun {name: 'ooptdd-payment333-prom4-20260715'})
UNWIND [
  {id:'REQ-PAY-OWNER-AUTH', event:'payment_owner_auth_verified', anchor:'ooptdd-payment333-owner-auth-20260715'},
  {id:'REQ-PAY-RESTART-LOCK', event:'payment_restart_safety_verified', anchor:'ooptdd-payment333-restart-lock-20260715'},
  {id:'REQ-PAY-DURABLE-REOPEN', event:'payment_durable_reopen_verified', anchor:'ooptdd-payment333-durable-reopen-20260715'},
  {id:'REQ-PAY-CONTEXT-EPOCH', event:'payment_context_epoch_binding_verified', anchor:'ooptdd-payment333-context-epoch-20260715'},
  {id:'REQ-PAY-ROTATION-LOCK-FENCE', event:'payment_rotation_lock_fence_verified', anchor:'ooptdd-payment333-rotation-lock-fence-20260715'},
  {id:'REQ-PAY-ROTATION-ESCROW', event:'payment_rotation_escrow_continuity_verified', anchor:'ooptdd-payment333-rotation-escrow-20260715'},
  {id:'REQ-PAY-UNIFIED-LANES', event:'payment_unified_lanes_supply_verified', anchor:'ooptdd-payment333-unified-lanes-20260715'}
] AS row
MERGE (req:OOPTDDRequirement:AbstractNode {name: row.id})
SET req.status = 'DONE',
    req.expected_event = row.event,
    req.gate_result = 'GREEN',
    req.cycle_id = run.cid,
    req.sourcePath = 'ooptdd/payment_requirements.yaml'
MERGE (site:ReferenceSite:Longinus {kg_anchor: row.anchor})
SET site.source_path = 'ooptdd/payment_conformance_adapter.py',
    site.symbol = 'run_payment_probe',
    site.emits = row.event,
    site.bound = true,
    site.blob_oid = '66fda9d7f0106cec274cbcd8cfbdb4889d36cde5',
    site.sha256 = 'e1bcb56bb4d7fa9d',
    site.cycle_id = run.cid
MERGE (run)-[:HAS_REQUIREMENT]->(req)
MERGE (req)-[:BOUND_AT]->(site)
MERGE (tree)-[:HAS_OOPTDD_REQUIREMENT]->(req);

MATCH (tree:LakatosTree {name: 'LakatosTree_333PaymentSafety_20260715'}),
      (cycle:PrometheusCycle {name: 'prom4-333-payment-safety-2026-07-15'})
MERGE (execution:OMDExecution:VerifiedArtifact {name: 'omd-payment333-prom4-task-a-20260715'})
SET execution.status = 'MERGED',
    execution.task_id = 'prom4-payment-ooptdd-omd',
    execution.agent_id = 'codex-root-v2',
    execution.orbit_id = 'orb-8215fb030d05',
    execution.fence = 1,
    execution.strict_writeset = true,
    execution.source_revision = 'b124bfa306fd5626b336dde5d0f7a05e1c272962',
    execution.task_commit = '977016ee6ddd14eb5c3935ebaabc2c039b22108b',
    execution.merge_commit = '444f32b5e041720258a269b4ae4293c9473ad6fc',
    execution.integration_generation = 1,
    execution.sourcePath = '333-payment/OMD_EXECUTION_RECEIPT_PROM4_PAYMENT_2026-07-15.json',
    execution.minioPath = 's3://docs/333/payment-safety/2026-07-15/OMD_EXECUTION_RECEIPT_PROM4_PAYMENT_2026-07-15.json',
    execution.verdict = 'MERGED_WITH_CRITICAL_OPERATIONAL_DEFECT_RECORDED'
MERGE (appraisal:OMDAppraisal:AbstractNode {name: 'omd-payment333-parallelism-appraisal-20260715'})
SET appraisal.verdict = 'SAFETY_PROGRESSIVE_THROUGHPUT_UNPROVEN',
    appraisal.highest_severity_problem = 'Default agent lease expiry force-removed a dirty worktree and destroyed uncommitted changes.',
    appraisal.optimization_status = 'Conflict-aware admission only; no cost/critical-path/agent-capability optimization or committed scaling benchmark.',
    appraisal.sourcePath = '333-payment/OMD_PARALLELISM_APPRAISAL_2026-07-15.md',
    appraisal.minioPath = 's3://docs/333/payment-safety/2026-07-15/OMD_PARALLELISM_APPRAISAL_2026-07-15.md'
MERGE (defect:OperationalDefect:AbstractNode {name: 'omd-dirty-worktree-destructive-reclaim-20260715'})
SET defect.status = 'OPEN',
    defect.severity = 'CRITICAL',
    defect.problem = 'A legitimate long-running agent exceeded the default 90-second TTL; reclaim retired the agent and force-removed its dirty worktree.',
    defect.required_fix = 'Quarantine or checkpoint dirty worktrees on expiry and expose recovery; never silently delete uncommitted state.',
    defect.falsifier = 'An expiry regression test preserves and recovers a dirty worktree without data loss.',
    defect.sourcePath = '333-payment/OMD_PARALLELISM_APPRAISAL_2026-07-15.md'
MERGE (tree)-[:EXECUTED_VIA]->(execution)
MERGE (cycle)-[:VERIFIED_BY]->(execution)
MERGE (tree)-[:HAS_METHOD_APPRAISAL]->(appraisal)
MERGE (appraisal)-[:EVALUATES]->(execution)
MERGE (appraisal)-[:IDENTIFIES]->(defect)
MERGE (execution)-[:EXPOSED]->(defect);

MATCH (execution:OMDExecution {name: 'omd-payment333-prom4-task-a-20260715'}),
      (concept {name: 'omd-dempsey-roll-2026-07-11'})
MERGE (execution)-[:APPLIES_CONCEPT]->(concept);

MATCH (tree:LakatosTree {name: 'LakatosTree_333PaymentSafety_20260715'})
SET tree.methodologies = ['PROM', 'Lakatos', 'OOPTDD_methodology_v1', 'OMD'],
    tree.updatedAt = '2026-07-15T12:50:00+09:00'
RETURN tree.name AS tree,
       size([(tree)-[:HAS_OOPTDD_RUN]->() | 1]) AS ooptdd_runs,
       size([(tree)-[:EXECUTED_VIA]->() | 1]) AS omd_executions,
       size([(tree)-[:HAS_METHOD_APPRAISAL]->() | 1]) AS method_appraisals;
