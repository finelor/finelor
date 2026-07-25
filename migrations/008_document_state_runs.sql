CREATE TABLE IF NOT EXISTS document_intake_runs (
  id INTEGER PRIMARY KEY,
  document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  run_status TEXT NOT NULL CHECK (run_status IN ('REQUESTED', 'RUNNING', 'COMPLETED', 'FAILED', 'CANCELLED')),
  failure_reason TEXT,
  metadata TEXT NOT NULL DEFAULT '{}',
  started_at TEXT,
  completed_at TEXT,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS document_accounting_runs (
  id INTEGER PRIMARY KEY,
  document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  run_kind TEXT NOT NULL CHECK (run_kind IN ('ACCOUNTING', 'VALIDATION', 'REVIEW', 'EXPORT')),
  run_status TEXT NOT NULL CHECK (run_status IN ('REQUESTED', 'RUNNING', 'COMPLETED', 'FAILED', 'CANCELLED')),
  provider TEXT,
  model_used TEXT,
  prompt_path TEXT,
  actor_type TEXT,
  action_type TEXT,
  export_batch_id INTEGER REFERENCES export_batches(id) ON DELETE SET NULL,
  failure_reason TEXT,
  metadata TEXT NOT NULL DEFAULT '{}',
  started_at TEXT,
  completed_at TEXT,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_document_intake_runs_document_created
ON document_intake_runs(document_id, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_document_accounting_runs_document_kind_created
ON document_accounting_runs(document_id, run_kind, created_at DESC, id DESC);

INSERT INTO document_intake_runs (
  document_id,
  run_status,
  failure_reason,
  metadata,
  started_at,
  completed_at,
  created_at,
  updated_at
)
SELECT
  dis.document_id,
  CASE
    WHEN dis.status = 'FAILED' THEN 'FAILED'
    WHEN dis.status = 'INGESTED' THEN 'COMPLETED'
    WHEN dis.status = 'PROCESSING' THEN 'RUNNING'
    ELSE 'REQUESTED'
  END,
  dis.failure_reason,
  '{}',
  dis.started_at,
  CASE
    WHEN dis.status = 'INGESTED' THEN dis.completed_at
    WHEN dis.status = 'FAILED' THEN COALESCE(dis.completed_at, dis.updated_at)
    ELSE NULL
  END,
  COALESCE(dis.started_at, dis.created_at, CURRENT_TIMESTAMP),
  COALESCE(dis.updated_at, CURRENT_TIMESTAMP)
FROM document_intake_state dis
WHERE NOT EXISTS (
  SELECT 1 FROM document_intake_runs dir WHERE dir.document_id = dis.document_id
);

INSERT INTO document_accounting_runs (
  document_id,
  run_kind,
  run_status,
  provider,
  model_used,
  prompt_path,
  actor_type,
  action_type,
  export_batch_id,
  failure_reason,
  metadata,
  started_at,
  completed_at,
  created_at,
  updated_at
)
SELECT
  das.document_id,
  CASE
    WHEN das.status IN ('REQUESTED', 'ACCOUNTING') THEN 'ACCOUNTING'
    WHEN das.status = 'VALIDATING' THEN 'VALIDATION'
    WHEN das.status = 'PENDING_REVIEW' THEN 'REVIEW'
    WHEN das.status IN ('READY_FOR_EXPORT', 'EXPORTING', 'EXPORTED') THEN 'EXPORT'
    WHEN das.status = 'FAILED' AND das.export_batch_id IS NOT NULL THEN 'EXPORT'
    WHEN das.status = 'FAILED' AND das.review_reason IS NOT NULL THEN 'REVIEW'
    WHEN das.status = 'FAILED' AND das.completed_at IS NOT NULL THEN 'VALIDATION'
    ELSE 'ACCOUNTING'
  END,
  CASE
    WHEN das.status = 'FAILED' THEN 'FAILED'
    WHEN das.status IN ('EXPORTED') THEN 'COMPLETED'
    WHEN das.status IN ('ACCOUNTING', 'VALIDATING', 'EXPORTING') THEN 'RUNNING'
    ELSE 'REQUESTED'
  END,
  NULL,
  NULL,
  NULL,
  CASE WHEN das.status = 'PENDING_REVIEW' THEN 'HUMAN' ELSE NULL END,
  CASE
    WHEN das.status = 'PENDING_REVIEW' THEN 'REQUEST'
    WHEN das.status IN ('READY_FOR_EXPORT', 'EXPORTING', 'EXPORTED') THEN 'EXPORT'
    ELSE NULL
  END,
  das.export_batch_id,
  das.failure_reason,
  '{}',
  COALESCE(das.started_at, das.requested_at),
  CASE
    WHEN das.status = 'EXPORTED' THEN das.completed_at
    WHEN das.status = 'FAILED' THEN COALESCE(das.completed_at, das.updated_at)
    ELSE NULL
  END,
  COALESCE(das.started_at, das.requested_at, das.created_at, CURRENT_TIMESTAMP),
  COALESCE(das.updated_at, CURRENT_TIMESTAMP)
FROM document_accounting_state das
WHERE das.status <> 'NOT_REQUESTED'
  AND NOT EXISTS (
    SELECT 1 FROM document_accounting_runs dar WHERE dar.document_id = das.document_id
  );
