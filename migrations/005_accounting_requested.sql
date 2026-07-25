ALTER TABLE documents
ADD COLUMN accounting_requested_at TEXT;

CREATE INDEX IF NOT EXISTS idx_documents_accounting_requested
ON documents(status, accounting_requested_at);
