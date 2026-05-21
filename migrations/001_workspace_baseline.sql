-- Finelor baseline schema

CREATE TABLE IF NOT EXISTS users (
  id INTEGER PRIMARY KEY,
  role TEXT NOT NULL DEFAULT 'admin' CHECK (role IN ('admin')),
  email TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  display_name TEXT,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_users_email ON users(email);

CREATE TABLE IF NOT EXISTS company_profile (
  singleton INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
  display_name TEXT,
  org_nr TEXT,
  jurisdiction TEXT NOT NULL DEFAULT 'SE',
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO company_profile (singleton)
VALUES (1)
ON CONFLICT (singleton) DO NOTHING;

CREATE TABLE IF NOT EXISTS documents (
  id INTEGER PRIMARY KEY,
  short_ref TEXT GENERATED ALWAYS AS ('D' || printf('%06d', id)) STORED UNIQUE,
  status TEXT NOT NULL DEFAULT 'RECEIVED' CHECK (status IN (
    'RECEIVED',
    'PROCESSING_VISION',
    'VISION_COMPLETE',
    'PROCESSING_ACCOUNTANT',
    'ACCOUNTANT_REVIEWED',
    'PROCESSING_VALIDATOR',
    'VALIDATED',
    'PENDING_HUMAN_REVIEW',
    'EXPORT_READY',
    'GENERATING_SIE4',
    'REVIEW_COMPLETED',
    'EXPORTED',
    'ARCHIVED',
    'FAILED'
  )),
  document_type TEXT,
  filename TEXT,
  original_path TEXT,
  file_hash TEXT NOT NULL UNIQUE,
  file_size_bytes INTEGER,
  mime_type TEXT,
  priority TEXT DEFAULT 'NORMAL' CHECK (priority IS NULL OR priority IN ('LOW', 'NORMAL', 'HIGH')),
  received_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  vision_started_at TEXT,
  vision_completed_at TEXT,
  accountant_reviewed_at TEXT,
  validated_at TEXT,
  review_completed_at TEXT,
  exported_at TEXT,
  exported_in_batch INTEGER REFERENCES export_batches(id) ON DELETE SET NULL,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS extracted_fields (
  id INTEGER PRIMARY KEY,
  document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  field_type TEXT NOT NULL,
  raw_value TEXT,
  parsed_value TEXT,
  parsed_type TEXT,
  confidence REAL,
  source TEXT,
  is_user_edited INTEGER NOT NULL DEFAULT 0 CHECK (is_user_edited IN (0, 1)),
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS accounting_decisions (
  id INTEGER PRIMARY KEY,
  document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  reasoning_text TEXT,
  assigned_account_code TEXT,
  account_name TEXT,
  vat_rate REAL,
  vat_amount REAL,
  net_amount REAL,
  gross_amount REAL,
  ai_confidence REAL,
  model_used TEXT,
  thinking_text TEXT,
  prompt_path TEXT,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS validation_results (
  id INTEGER PRIMARY KEY,
  document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  checks_passed INTEGER DEFAULT 0,
  checks_failed INTEGER DEFAULT 0,
  validation_errors TEXT,
  overall_status TEXT,
  checked_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS review_decisions (
  id INTEGER PRIMARY KEY,
  document_id INTEGER NOT NULL UNIQUE REFERENCES documents(id) ON DELETE CASCADE,
  confidence_score REAL,
  decision_type TEXT,
  human_review_required INTEGER NOT NULL DEFAULT 0 CHECK (human_review_required IN (0, 1)),
  review_reason TEXT,
  reviewed_by TEXT,
  corrections TEXT,
  reviewed_at TEXT DEFAULT CURRENT_TIMESTAMP,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS export_batches (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL REFERENCES users(id),
  document_count INTEGER,
  filter_criteria TEXT,
  sie4_path TEXT,
  manifest_path TEXT,
  zip_path TEXT,
  expires_at TEXT,
  generated_at TEXT DEFAULT CURRENT_TIMESTAMP,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS invoices (
  id INTEGER PRIMARY KEY,
  document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  supplier_name TEXT,
  invoice_number TEXT,
  invoice_date TEXT,
  total_amount REAL,
  vat_amount REAL,
  currency TEXT DEFAULT 'SEK',
  status TEXT DEFAULT 'PENDING_ANALYSIS',
  bas_year INTEGER DEFAULT 2026,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS account_assignments (
  id INTEGER PRIMARY KEY,
  invoice_id INTEGER NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
  account_code TEXT NOT NULL,
  account_name TEXT,
  description TEXT,
  amount REAL NOT NULL,
  vat_code TEXT,
  confidence REAL,
  reasoning TEXT,
  sort_order INTEGER DEFAULT 0,
  is_debit INTEGER NOT NULL DEFAULT 1 CHECK (is_debit IN (0, 1)),
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS document_events (
  id INTEGER PRIMARY KEY,
  document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  event_type TEXT NOT NULL,
  payload TEXT,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS channel_identities (
  id INTEGER PRIMARY KEY,
  channel_type TEXT NOT NULL,
  channel_identifier TEXT NOT NULL,
  metadata TEXT,
  active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS document_artifacts (
  id INTEGER PRIMARY KEY,
  document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  channel_type TEXT NOT NULL,
  channel_identifier TEXT NOT NULL,
  profile_identifier TEXT,
  external_artifact_id TEXT,
  source_timestamp TEXT,
  original_filename TEXT,
  mime_type TEXT,
  file_hash TEXT,
  metadata TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS document_interactions (
  id INTEGER PRIMARY KEY,
  document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  channel_type TEXT NOT NULL,
  channel_identifier TEXT NOT NULL,
  profile_identifier TEXT,
  interaction_kind TEXT NOT NULL DEFAULT 'HUMAN_REVIEW',
  pending_action TEXT NOT NULL DEFAULT 'EDIT_FIELD',
  status TEXT NOT NULL DEFAULT 'PENDING' CHECK (status IN ('PENDING', 'COMPLETED', 'CANCELLED')),
  payload TEXT NOT NULL DEFAULT '{}',
  expires_at TEXT,
  completed_at TEXT,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS agent_sessions (
  id INTEGER PRIMARY KEY,
  session_key TEXT NOT NULL UNIQUE,
  channel_type TEXT NOT NULL,
  channel_identifier TEXT NOT NULL,
  started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  last_active_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  metadata TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS agent_messages (
  id INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
  turn_index INTEGER NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
  content TEXT NOT NULL,
  source_message_id TEXT,
  profile_identifier TEXT,
  metadata TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_documents_status ON documents(status);
CREATE INDEX IF NOT EXISTS idx_documents_exported_batch ON documents(exported_in_batch);
CREATE INDEX IF NOT EXISTS idx_documents_received_at ON documents(received_at DESC);
CREATE INDEX IF NOT EXISTS idx_extracted_fields_document ON extracted_fields(document_id);
CREATE INDEX IF NOT EXISTS idx_export_batches_user ON export_batches(user_id);
CREATE INDEX IF NOT EXISTS idx_invoices_document_id ON invoices(document_id);
CREATE INDEX IF NOT EXISTS idx_invoices_status ON invoices(status);
CREATE INDEX IF NOT EXISTS idx_invoices_invoice_date ON invoices(invoice_date);
CREATE INDEX IF NOT EXISTS idx_account_assignments_invoice ON account_assignments(invoice_id);
CREATE INDEX IF NOT EXISTS idx_account_assignments_account_code ON account_assignments(account_code);
CREATE INDEX IF NOT EXISTS idx_document_events_document ON document_events(document_id);
CREATE INDEX IF NOT EXISTS idx_document_events_type ON document_events(event_type);
CREATE INDEX IF NOT EXISTS idx_document_events_created ON document_events(created_at);
CREATE INDEX IF NOT EXISTS idx_channel_identities_channel ON channel_identities(channel_type, channel_identifier);
CREATE INDEX IF NOT EXISTS idx_document_artifacts_document_id ON document_artifacts(document_id);
CREATE INDEX IF NOT EXISTS idx_document_artifacts_channel ON document_artifacts(channel_type, channel_identifier);
CREATE INDEX IF NOT EXISTS idx_document_artifacts_profile ON document_artifacts(channel_type, profile_identifier) WHERE profile_identifier IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_document_artifacts_external ON document_artifacts(channel_type, external_artifact_id) WHERE external_artifact_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_document_interactions_document ON document_interactions(document_id);
CREATE INDEX IF NOT EXISTS idx_document_interactions_channel_pending ON document_interactions(channel_type, channel_identifier, profile_identifier, completed_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_document_interactions_pending_actor ON document_interactions(channel_type, channel_identifier, COALESCE(profile_identifier, ''), interaction_kind) WHERE completed_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_agent_sessions_last_active ON agent_sessions(last_active_at DESC);
CREATE INDEX IF NOT EXISTS idx_agent_messages_recent ON agent_messages(created_at DESC);
CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_messages_session_turn_role ON agent_messages(session_id, turn_index, role);
