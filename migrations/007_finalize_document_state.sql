DROP TRIGGER IF EXISTS trg_documents_document_state_after_insert;
DROP TRIGGER IF EXISTS trg_documents_document_state_after_update;

DROP INDEX IF EXISTS idx_documents_accounting_requested;
DROP INDEX IF EXISTS idx_documents_status;
DROP INDEX IF EXISTS idx_documents_exported_batch;

ALTER TABLE documents DROP COLUMN status;
ALTER TABLE documents DROP COLUMN vision_started_at;
ALTER TABLE documents DROP COLUMN vision_completed_at;
ALTER TABLE documents DROP COLUMN accountant_reviewed_at;
ALTER TABLE documents DROP COLUMN validated_at;
ALTER TABLE documents DROP COLUMN review_completed_at;
ALTER TABLE documents DROP COLUMN exported_at;
ALTER TABLE documents DROP COLUMN exported_in_batch;
ALTER TABLE documents DROP COLUMN accounting_requested_at;
