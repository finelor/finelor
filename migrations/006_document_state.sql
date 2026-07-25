CREATE TABLE IF NOT EXISTS document_intake_state (
  id INTEGER PRIMARY KEY,
  document_id INTEGER NOT NULL UNIQUE REFERENCES documents(id) ON DELETE CASCADE,
  status TEXT NOT NULL CHECK (status IN ('RECEIVED', 'PROCESSING', 'INGESTED', 'FAILED')),
  failure_reason TEXT,
  started_at TEXT,
  completed_at TEXT,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS document_accounting_state (
  id INTEGER PRIMARY KEY,
  document_id INTEGER NOT NULL UNIQUE REFERENCES documents(id) ON DELETE CASCADE,
  status TEXT NOT NULL CHECK (status IN (
    'NOT_REQUESTED',
    'REQUESTED',
    'ACCOUNTING',
    'VALIDATING',
    'PENDING_REVIEW',
    'READY_FOR_EXPORT',
    'EXPORTING',
    'EXPORTED',
    'FAILED'
  )),
  requested_at TEXT,
  started_at TEXT,
  completed_at TEXT,
  failure_reason TEXT,
  review_reason TEXT,
  export_batch_id INTEGER REFERENCES export_batches(id) ON DELETE SET NULL,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_document_intake_state_status
ON document_intake_state(status);

CREATE INDEX IF NOT EXISTS idx_document_accounting_state_status
ON document_accounting_state(status, requested_at, export_batch_id);

INSERT OR IGNORE INTO document_intake_state (
  document_id,
  status,
  failure_reason,
  started_at,
  completed_at,
  created_at,
  updated_at
)
SELECT
  d.id,
  CASE
    WHEN d.status = 'RECEIVED' THEN 'RECEIVED'
    WHEN d.status = 'PROCESSING_VISION' THEN 'PROCESSING'
    WHEN d.status = 'FAILED'
      AND d.accounting_requested_at IS NULL
      AND d.accountant_reviewed_at IS NULL
      AND d.validated_at IS NULL
      AND d.review_completed_at IS NULL
      AND d.exported_at IS NULL
      AND d.exported_in_batch IS NULL
    THEN 'FAILED'
    ELSE 'INGESTED'
  END,
  CASE
    WHEN d.status = 'FAILED'
      AND d.accounting_requested_at IS NULL
      AND d.accountant_reviewed_at IS NULL
      AND d.validated_at IS NULL
      AND d.review_completed_at IS NULL
      AND d.exported_at IS NULL
      AND d.exported_in_batch IS NULL
    THEN 'legacy_failed_status'
    ELSE NULL
  END,
  d.vision_started_at,
  d.vision_completed_at,
  d.created_at,
  d.updated_at
FROM documents d;

INSERT OR IGNORE INTO document_accounting_state (
  document_id,
  status,
  requested_at,
  started_at,
  completed_at,
  failure_reason,
  review_reason,
  export_batch_id,
  created_at,
  updated_at
)
SELECT
  d.id,
  CASE
    WHEN d.status IN ('RECEIVED', 'PROCESSING_VISION') THEN 'NOT_REQUESTED'
    WHEN d.status = 'VISION_COMPLETE' AND d.accounting_requested_at IS NULL THEN 'NOT_REQUESTED'
    WHEN d.status = 'VISION_COMPLETE' AND d.accounting_requested_at IS NOT NULL THEN 'REQUESTED'
    WHEN d.status IN ('PROCESSING_ACCOUNTANT', 'ACCOUNTANT_REVIEWED') THEN 'ACCOUNTING'
    WHEN d.status IN ('PROCESSING_VALIDATOR', 'VALIDATED') THEN 'VALIDATING'
    WHEN d.status = 'PENDING_HUMAN_REVIEW' THEN 'PENDING_REVIEW'
    WHEN d.status IN ('EXPORT_READY', 'REVIEW_COMPLETED') THEN 'READY_FOR_EXPORT'
    WHEN d.status = 'GENERATING_SIE4' THEN 'EXPORTING'
    WHEN d.status IN ('EXPORTED', 'ARCHIVED') THEN 'EXPORTED'
    WHEN d.status = 'FAILED'
      AND (
        d.accounting_requested_at IS NOT NULL
        OR d.accountant_reviewed_at IS NOT NULL
        OR d.validated_at IS NOT NULL
        OR d.review_completed_at IS NOT NULL
        OR d.exported_at IS NOT NULL
        OR d.exported_in_batch IS NOT NULL
      )
    THEN 'FAILED'
    ELSE 'NOT_REQUESTED'
  END,
  d.accounting_requested_at,
  COALESCE(d.accounting_requested_at, d.accountant_reviewed_at, d.validated_at, d.review_completed_at, d.exported_at),
  CASE
    WHEN d.status IN ('EXPORT_READY', 'GENERATING_SIE4', 'REVIEW_COMPLETED', 'EXPORTED', 'ARCHIVED')
      THEN COALESCE(d.exported_at, d.review_completed_at, d.validated_at, d.accountant_reviewed_at, d.updated_at)
    WHEN d.status = 'FAILED'
      AND (
        d.accounting_requested_at IS NOT NULL
        OR d.accountant_reviewed_at IS NOT NULL
        OR d.validated_at IS NOT NULL
        OR d.review_completed_at IS NOT NULL
        OR d.exported_at IS NOT NULL
        OR d.exported_in_batch IS NOT NULL
      )
    THEN COALESCE(d.exported_at, d.review_completed_at, d.validated_at, d.accountant_reviewed_at, d.updated_at)
    ELSE NULL
  END,
  CASE
    WHEN d.status = 'FAILED'
      AND (
        d.accounting_requested_at IS NOT NULL
        OR d.accountant_reviewed_at IS NOT NULL
        OR d.validated_at IS NOT NULL
        OR d.review_completed_at IS NOT NULL
        OR d.exported_at IS NOT NULL
        OR d.exported_in_batch IS NOT NULL
      )
    THEN 'legacy_failed_status'
    ELSE NULL
  END,
  NULL,
  d.exported_in_batch,
  d.created_at,
  d.updated_at
FROM documents d;

CREATE TRIGGER IF NOT EXISTS trg_documents_document_state_after_insert
AFTER INSERT ON documents
BEGIN
  INSERT OR IGNORE INTO document_intake_state (
    document_id, status, started_at, completed_at, created_at, updated_at
  )
  VALUES (
    NEW.id,
    CASE
      WHEN NEW.status = 'RECEIVED' THEN 'RECEIVED'
      WHEN NEW.status = 'PROCESSING_VISION' THEN 'PROCESSING'
      WHEN NEW.status = 'FAILED'
        AND NEW.accounting_requested_at IS NULL
        AND NEW.accountant_reviewed_at IS NULL
        AND NEW.validated_at IS NULL
        AND NEW.review_completed_at IS NULL
        AND NEW.exported_at IS NULL
        AND NEW.exported_in_batch IS NULL
      THEN 'FAILED'
      ELSE 'INGESTED'
    END,
    NEW.vision_started_at,
    NEW.vision_completed_at,
    COALESCE(NEW.created_at, CURRENT_TIMESTAMP),
    COALESCE(NEW.updated_at, CURRENT_TIMESTAMP)
  );

  INSERT OR IGNORE INTO document_accounting_state (
    document_id, status, requested_at, started_at, completed_at, failure_reason, review_reason, export_batch_id, created_at, updated_at
  )
  VALUES (
    NEW.id,
    CASE
      WHEN NEW.status IN ('RECEIVED', 'PROCESSING_VISION') THEN 'NOT_REQUESTED'
      WHEN NEW.status = 'VISION_COMPLETE' AND NEW.accounting_requested_at IS NULL THEN 'NOT_REQUESTED'
      WHEN NEW.status = 'VISION_COMPLETE' AND NEW.accounting_requested_at IS NOT NULL THEN 'REQUESTED'
      WHEN NEW.status IN ('PROCESSING_ACCOUNTANT', 'ACCOUNTANT_REVIEWED') THEN 'ACCOUNTING'
      WHEN NEW.status IN ('PROCESSING_VALIDATOR', 'VALIDATED') THEN 'VALIDATING'
      WHEN NEW.status = 'PENDING_HUMAN_REVIEW' THEN 'PENDING_REVIEW'
      WHEN NEW.status IN ('EXPORT_READY', 'REVIEW_COMPLETED') THEN 'READY_FOR_EXPORT'
      WHEN NEW.status = 'GENERATING_SIE4' THEN 'EXPORTING'
      WHEN NEW.status IN ('EXPORTED', 'ARCHIVED') THEN 'EXPORTED'
      WHEN NEW.status = 'FAILED'
        AND (
          NEW.accounting_requested_at IS NOT NULL
          OR NEW.accountant_reviewed_at IS NOT NULL
          OR NEW.validated_at IS NOT NULL
          OR NEW.review_completed_at IS NOT NULL
          OR NEW.exported_at IS NOT NULL
          OR NEW.exported_in_batch IS NOT NULL
        )
      THEN 'FAILED'
      ELSE 'NOT_REQUESTED'
    END,
    NEW.accounting_requested_at,
    COALESCE(NEW.accounting_requested_at, NEW.accountant_reviewed_at, NEW.validated_at, NEW.review_completed_at, NEW.exported_at),
    CASE
      WHEN NEW.status IN ('EXPORT_READY', 'GENERATING_SIE4', 'REVIEW_COMPLETED', 'EXPORTED', 'ARCHIVED')
        THEN COALESCE(NEW.exported_at, NEW.review_completed_at, NEW.validated_at, NEW.accountant_reviewed_at, NEW.updated_at, CURRENT_TIMESTAMP)
      WHEN NEW.status = 'FAILED'
        AND (
          NEW.accounting_requested_at IS NOT NULL
          OR NEW.accountant_reviewed_at IS NOT NULL
          OR NEW.validated_at IS NOT NULL
          OR NEW.review_completed_at IS NOT NULL
          OR NEW.exported_at IS NOT NULL
          OR NEW.exported_in_batch IS NOT NULL
        )
      THEN COALESCE(NEW.exported_at, NEW.review_completed_at, NEW.validated_at, NEW.accountant_reviewed_at, NEW.updated_at, CURRENT_TIMESTAMP)
      ELSE NULL
    END,
    CASE
      WHEN NEW.status = 'FAILED'
        AND (
          NEW.accounting_requested_at IS NOT NULL
          OR NEW.accountant_reviewed_at IS NOT NULL
          OR NEW.validated_at IS NOT NULL
          OR NEW.review_completed_at IS NOT NULL
          OR NEW.exported_at IS NOT NULL
          OR NEW.exported_in_batch IS NOT NULL
        )
      THEN 'legacy_failed_status'
      ELSE NULL
    END,
    NULL,
    NEW.exported_in_batch,
    COALESCE(NEW.created_at, CURRENT_TIMESTAMP),
    COALESCE(NEW.updated_at, CURRENT_TIMESTAMP)
  );
END;

CREATE TRIGGER IF NOT EXISTS trg_documents_document_state_after_update
AFTER UPDATE OF status, accounting_requested_at, vision_started_at, vision_completed_at, accountant_reviewed_at, validated_at, review_completed_at, exported_at, exported_in_batch, updated_at ON documents
BEGIN
  UPDATE document_intake_state
  SET
    status = CASE
      WHEN NEW.status = 'RECEIVED' THEN 'RECEIVED'
      WHEN NEW.status = 'PROCESSING_VISION' THEN 'PROCESSING'
      WHEN NEW.status = 'FAILED'
        AND NEW.accounting_requested_at IS NULL
        AND NEW.accountant_reviewed_at IS NULL
        AND NEW.validated_at IS NULL
        AND NEW.review_completed_at IS NULL
        AND NEW.exported_at IS NULL
        AND NEW.exported_in_batch IS NULL
      THEN 'FAILED'
      ELSE 'INGESTED'
    END,
    failure_reason = CASE
      WHEN NEW.status = 'FAILED'
        AND NEW.accounting_requested_at IS NULL
        AND NEW.accountant_reviewed_at IS NULL
        AND NEW.validated_at IS NULL
        AND NEW.review_completed_at IS NULL
        AND NEW.exported_at IS NULL
        AND NEW.exported_in_batch IS NULL
      THEN COALESCE(failure_reason, 'legacy_failed_status')
      ELSE NULL
    END,
    started_at = NEW.vision_started_at,
    completed_at = NEW.vision_completed_at,
    updated_at = COALESCE(NEW.updated_at, CURRENT_TIMESTAMP)
  WHERE document_id = NEW.id;

  UPDATE document_accounting_state
  SET
    status = CASE
      WHEN NEW.status IN ('RECEIVED', 'PROCESSING_VISION') THEN 'NOT_REQUESTED'
      WHEN NEW.status = 'VISION_COMPLETE' AND NEW.accounting_requested_at IS NULL THEN 'NOT_REQUESTED'
      WHEN NEW.status = 'VISION_COMPLETE' AND NEW.accounting_requested_at IS NOT NULL THEN 'REQUESTED'
      WHEN NEW.status IN ('PROCESSING_ACCOUNTANT', 'ACCOUNTANT_REVIEWED') THEN 'ACCOUNTING'
      WHEN NEW.status IN ('PROCESSING_VALIDATOR', 'VALIDATED') THEN 'VALIDATING'
      WHEN NEW.status = 'PENDING_HUMAN_REVIEW' THEN 'PENDING_REVIEW'
      WHEN NEW.status IN ('EXPORT_READY', 'REVIEW_COMPLETED') THEN 'READY_FOR_EXPORT'
      WHEN NEW.status = 'GENERATING_SIE4' THEN 'EXPORTING'
      WHEN NEW.status IN ('EXPORTED', 'ARCHIVED') THEN 'EXPORTED'
      WHEN NEW.status = 'FAILED'
        AND (
          NEW.accounting_requested_at IS NOT NULL
          OR NEW.accountant_reviewed_at IS NOT NULL
          OR NEW.validated_at IS NOT NULL
          OR NEW.review_completed_at IS NOT NULL
          OR NEW.exported_at IS NOT NULL
          OR NEW.exported_in_batch IS NOT NULL
        )
      THEN 'FAILED'
      ELSE 'NOT_REQUESTED'
    END,
    requested_at = NEW.accounting_requested_at,
    started_at = COALESCE(NEW.accounting_requested_at, NEW.accountant_reviewed_at, NEW.validated_at, NEW.review_completed_at, NEW.exported_at),
    completed_at = CASE
      WHEN NEW.status IN ('EXPORT_READY', 'GENERATING_SIE4', 'REVIEW_COMPLETED', 'EXPORTED', 'ARCHIVED')
        THEN COALESCE(NEW.exported_at, NEW.review_completed_at, NEW.validated_at, NEW.accountant_reviewed_at, NEW.updated_at)
      WHEN NEW.status = 'FAILED'
        AND (
          NEW.accounting_requested_at IS NOT NULL
          OR NEW.accountant_reviewed_at IS NOT NULL
          OR NEW.validated_at IS NOT NULL
          OR NEW.review_completed_at IS NOT NULL
          OR NEW.exported_at IS NOT NULL
          OR NEW.exported_in_batch IS NOT NULL
        )
      THEN COALESCE(NEW.exported_at, NEW.review_completed_at, NEW.validated_at, NEW.accountant_reviewed_at, NEW.updated_at)
      ELSE NULL
    END,
    failure_reason = CASE
      WHEN NEW.status = 'FAILED'
        AND (
          NEW.accounting_requested_at IS NOT NULL
          OR NEW.accountant_reviewed_at IS NOT NULL
          OR NEW.validated_at IS NOT NULL
          OR NEW.review_completed_at IS NOT NULL
          OR NEW.exported_at IS NOT NULL
          OR NEW.exported_in_batch IS NOT NULL
        )
      THEN COALESCE(failure_reason, 'legacy_failed_status')
      ELSE NULL
    END,
    export_batch_id = NEW.exported_in_batch,
    updated_at = COALESCE(NEW.updated_at, CURRENT_TIMESTAMP)
  WHERE document_id = NEW.id;
END;
