// Idempotent Longinus registration for PROM 4 / OOPTDD / OMD artifacts.
// The binding set pins sourceId + path + symbol/line range + sha256 + git blob/commit.

MATCH (tree:LakatosTree {name: 'LakatosTree_333PaymentSafety_20260715'})
MERGE (bindingSet:LonginusBindingSet:VerifiedArtifact {name: 'longinus-payment333-method-artifacts-20260715'})
SET bindingSet.status = 'COMPLETE',
    bindingSet.binding_count = 9,
    bindingSet.bound_count = 9,
    bindingSet.drift_count = 0,
    bindingSet.source_commit = 'fc05dd24b729088270efa35d7baad72e55e7e79d',
    bindingSet.repository = 'ssh://lagyeongjun@bhgman.iptime.org/Users/lagyeongjun/CD/333',
    bindingSet.sourcePath = '333-payment/LONGINUS_PROM4_METHOD_BINDINGS_2026-07-15.json',
    bindingSet.minioPath = 's3://docs/333/payment-safety/2026-07-15/LONGINUS_PROM4_METHOD_BINDINGS_2026-07-15.json',
    bindingSet.verifiedAt = '2026-07-15T13:14:35+09:00'
MERGE (tree)-[:HAS_LONGINUS_BINDING_SET]->(bindingSet)
SET tree.source_commit = 'fc05dd24b729088270efa35d7baad72e55e7e79d',
    tree.longinus_status = 'BOUND',
    tree.longinus_binding_count = 16,
    tree.longinus_drift_count = 0,
    tree.updatedAt = '2026-07-15T13:14:35+09:00';

MATCH (tree:LakatosTree {name: 'LakatosTree_333PaymentSafety_20260715'}),
      (bindingSet:LonginusBindingSet {name: 'longinus-payment333-method-artifacts-20260715'})
UNWIND [
  {kg_anchor:'longinus-prom4-payment-safety-report-20260715', sourceId:'prom4-333-payment-safety-2026-07-15', sourcePath:'333-payment/PROM_4_PAYMENT_SAFETY_2026-07-15.md', symbol:'document', line_start:1, line_end:192, sha256:'58e5ad22d4242ff9cf73d188be5ce3d4cc764fc33f446f0bddd2095b71df07d4', blob_oid:'73cf35352a4cf5fa66e909ba114cc9e473b2edb8', minioPath:'s3://docs/333/payment-safety/2026-07-15/PROM_4_PAYMENT_SAFETY_2026-07-15.md', method:'PROM', graph_target:'prom4-333-payment-safety-2026-07-15'},
  {kg_anchor:'longinus-ooptdd-payment-spec-20260715', sourceId:'payment333-safety-conformance', sourcePath:'ooptdd/payment_requirements.yaml', symbol:'payment333-safety-conformance', line_start:1, line_end:144, sha256:'330cfc3b6feb048cad25eb11ec1d46b45ff91a7d2f37ff682a2beaab030bd212', blob_oid:'f84ffbcc6eaaf82c42182a75aff8a86a199f118d', minioPath:'s3://docs/333/payment-safety/2026-07-15/payment_requirements.yaml', method:'OOPTDD', graph_target:'ooptdd-payment333-prom4-20260715'},
  {kg_anchor:'longinus-ooptdd-payment-adapter-20260715', sourceId:'payment_conformance_adapter.run_payment_probe', sourcePath:'ooptdd/payment_conformance_adapter.py', symbol:'run_payment_probe', line_start:75, line_end:108, sha256:'fe51e3d75da6329b4bfa324faacfc0d1d6642fe8dfa83d98dec5870ed16d2b56', blob_oid:'e5620026438896ffd3113ff45d880f1d2052b60f', minioPath:'s3://docs/333/payment-safety/2026-07-15/payment_conformance_adapter.py', method:'OOPTDD', graph_target:'ooptdd-payment333-prom4-20260715'},
  {kg_anchor:'longinus-ooptdd-payment-runner-20260715', sourceId:'payment_run.main', sourcePath:'ooptdd/payment_run.py', symbol:'main', line_start:57, line_end:84, sha256:'70d3e254d29f1c08709dc718e53d908d6bdec533c8c3a24201a6906b12459eeb', blob_oid:'303bd2aa35474d70cbba85e0fb9fd7a9184af67d', minioPath:'s3://docs/333/payment-safety/2026-07-15/payment_run.py', method:'OOPTDD', graph_target:'ooptdd-payment333-prom4-20260715'},
  {kg_anchor:'longinus-ooptdd-payment-receipt-20260715', sourceId:'payment333-prom4-ooptdd-20260715', sourcePath:'333-payment/OOPTDD_RECEIPT_PROM4_PAYMENT_2026-07-15.json', symbol:'payment333-ooptdd-receipt-v1', line_start:1, line_end:407, sha256:'13f653f3d77ee6aacfcb89913d7efd34097744ea52b095b87dbbdba19d69b256', blob_oid:'b318ce2029c17b6aacefc907bc1d0aa3ab334259', minioPath:'s3://docs/333/payment-safety/2026-07-15/OOPTDD_RECEIPT_PROM4_PAYMENT_2026-07-15.json', method:'OOPTDD', graph_target:'ooptdd-payment333-prom4-20260715'},
  {kg_anchor:'longinus-omd-payment-work-unit-20260715', sourceId:'prom4-payment-ooptdd-omd', sourcePath:'333-payment/OMD_WORK_UNIT_PROM4_PAYMENT_METHODS.yaml', symbol:'omd-work-unit-v1', line_start:1, line_end:25, sha256:'917213c37924ba524791ae7dba816a211ba7fbc1d832ef0083f0f7d406491bb8', blob_oid:'2fc1d60ab9247cbf6ac8b54b51fe2ac01e5636d4', minioPath:'s3://docs/333/payment-safety/2026-07-15/OMD_WORK_UNIT_PROM4_PAYMENT_METHODS.yaml', method:'OMD', graph_target:'omd-payment333-prom4-task-a-20260715'},
  {kg_anchor:'longinus-omd-payment-execution-receipt-20260715', sourceId:'payment333-omd-execution-receipt-v1', sourcePath:'333-payment/OMD_EXECUTION_RECEIPT_PROM4_PAYMENT_2026-07-15.json', symbol:'payment333-omd-execution-receipt-v1', line_start:1, line_end:63, sha256:'7b4486ad01b23389bc5e6c4877d8c9213f572ab9806641143079343ae0691c61', blob_oid:'d0d374f5ccaccc42c60ffea30808fb26c7b09450', minioPath:'s3://docs/333/payment-safety/2026-07-15/OMD_EXECUTION_RECEIPT_PROM4_PAYMENT_2026-07-15.json', method:'OMD', graph_target:'omd-payment333-prom4-task-a-20260715'},
  {kg_anchor:'longinus-omd-payment-appraisal-20260715', sourceId:'omd-payment333-parallelism-appraisal-20260715', sourcePath:'333-payment/OMD_PARALLELISM_APPRAISAL_2026-07-15.md', symbol:'document', line_start:1, line_end:71, sha256:'d404b166d94fd2eab16d4fccbcdc8ca9f8a7e49bf44450350faee8799d1e38ad', blob_oid:'d96100ca69806add2d09eda536624e989fd0425d', minioPath:'s3://docs/333/payment-safety/2026-07-15/OMD_PARALLELISM_APPRAISAL_2026-07-15.md', method:'OMD', graph_target:'omd-payment333-parallelism-appraisal-20260715'},
  {kg_anchor:'longinus-ooptdd-omd-application-20260715', sourceId:'ooptdd-omd-payment333-application-20260715', sourcePath:'333-payment/OOPTDD_OMD_APPLICATION_2026-07-15.md', symbol:'document', line_start:1, line_end:70, sha256:'e86871c4a591f4dbdb0872be4ed3cb1c08ecd262914a100cad35070e4c914a8a', blob_oid:'5db71255356dfbe3fb4fc6a4f6ee615e857a93f5', minioPath:'s3://docs/333/payment-safety/2026-07-15/OOPTDD_OMD_APPLICATION_2026-07-15.md', method:'OOPTDD+OMD', graph_target:'omd-payment333-prom4-task-a-20260715'}
] AS row
MERGE (site:ReferenceSite:Longinus {kg_anchor: row.kg_anchor})
SET site.sourceId = row.sourceId,
    site.source_id = row.sourceId,
    site.sourcePath = row.sourcePath,
    site.source_path = row.sourcePath,
    site.repo_relpath = row.sourcePath,
    site.symbol = row.symbol,
    site.line_start = row.line_start,
    site.line_end = row.line_end,
    site.sha256 = row.sha256,
    site.blob_oid = row.blob_oid,
    site.commit = 'fc05dd24b729088270efa35d7baad72e55e7e79d',
    site.repository = 'ssh://lagyeongjun@bhgman.iptime.org/Users/lagyeongjun/CD/333',
    site.minioPath = row.minioPath,
    site.method = row.method,
    site.graph_target = row.graph_target,
    site.bound = true,
    site.binding_state = 'BOUND',
    site.drift_state = 'CLEAN',
    site.verifiedAt = '2026-07-15T13:14:35+09:00'
MERGE (bindingSet)-[:HAS_REFERENCE_SITE]->(site)
MERGE (tree)-[:HAS_LONGINUS_REFERENCE]->(site);

MATCH (cycle:PrometheusCycle {name: 'prom4-333-payment-safety-2026-07-15'}),
      (site:ReferenceSite {kg_anchor: 'longinus-prom4-payment-safety-report-20260715'})
MERGE (cycle)-[:DOCUMENTED_AT]->(site);

MATCH (run:OOPTDDRun {name: 'ooptdd-payment333-prom4-20260715'})
MATCH (site:ReferenceSite)
WHERE site.kg_anchor IN [
  'longinus-ooptdd-payment-spec-20260715',
  'longinus-ooptdd-payment-adapter-20260715',
  'longinus-ooptdd-payment-runner-20260715',
  'longinus-ooptdd-payment-receipt-20260715'
]
MERGE (run)-[:GROUNDED_AT]->(site);

MATCH (execution:OMDExecution {name: 'omd-payment333-prom4-task-a-20260715'})
MATCH (site:ReferenceSite)
WHERE site.kg_anchor IN [
  'longinus-omd-payment-work-unit-20260715',
  'longinus-omd-payment-execution-receipt-20260715',
  'longinus-ooptdd-omd-application-20260715'
]
MERGE (execution)-[:GROUNDED_AT]->(site);

MATCH (appraisal:OMDAppraisal {name: 'omd-payment333-parallelism-appraisal-20260715'}),
      (site:ReferenceSite {kg_anchor: 'longinus-omd-payment-appraisal-20260715'})
MERGE (appraisal)-[:GROUNDED_AT]->(site);

MATCH (run:OOPTDDRun {name: 'ooptdd-payment333-prom4-20260715'})-[:HAS_REQUIREMENT]->
      (req:OOPTDDRequirement)-[:BOUND_AT]->(site:ReferenceSite)
SET site.sourceId = req.name,
    site.source_id = req.name,
    site.sourcePath = 'ooptdd/payment_conformance_adapter.py',
    site.source_path = 'ooptdd/payment_conformance_adapter.py',
    site.repo_relpath = 'ooptdd/payment_conformance_adapter.py',
    site.symbol = 'run_payment_probe',
    site.line_start = 75,
    site.line_end = 108,
    site.sha256 = 'fe51e3d75da6329b4bfa324faacfc0d1d6642fe8dfa83d98dec5870ed16d2b56',
    site.blob_oid = 'e5620026438896ffd3113ff45d880f1d2052b60f',
    site.commit = 'fc05dd24b729088270efa35d7baad72e55e7e79d',
    site.repository = 'ssh://lagyeongjun@bhgman.iptime.org/Users/lagyeongjun/CD/333',
    site.minioPath = 's3://docs/333/payment-safety/2026-07-15/payment_conformance_adapter.py',
    site.method = 'OOPTDD',
    site.bound = true,
    site.binding_state = 'BOUND',
    site.drift_state = 'CLEAN',
    site.verifiedAt = '2026-07-15T13:14:35+09:00';

MATCH (bindingSet:LonginusBindingSet {name: 'longinus-payment333-method-artifacts-20260715'})
OPTIONAL MATCH (bindingSet)-[:HAS_REFERENCE_SITE]->(site:ReferenceSite)
RETURN bindingSet.name AS binding_set,
       bindingSet.status AS status,
       count(site) AS artifact_bindings,
       count(CASE WHEN site.bound = true AND site.drift_state = 'CLEAN' THEN 1 END) AS clean_bindings;
