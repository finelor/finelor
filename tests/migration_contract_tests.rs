mod common;

#[test]
fn workspace_baseline_migration_contains_workspace_only_contracts() {
    let migration = include_str!("../migrations/001_workspace_baseline.sql");

    assert!(!migration.contains("workspace_id"));
    assert!(!migration.contains("company_id"));
    assert!(!migration.contains("CREATE TABLE IF NOT EXISTS companies"));
    assert!(!migration.contains("finelor_schema_migrations"));
    assert!(migration.contains("CREATE TABLE IF NOT EXISTS channel_identities"));
    assert!(migration.contains("CREATE TABLE IF NOT EXISTS document_artifacts"));
    assert!(migration.contains("CREATE TABLE IF NOT EXISTS agent_sessions"));
    assert!(migration.contains("idx_agent_messages_session_turn_role"));
    assert!(!migration.contains("idx_agent_messages_session_turn ON agent_messages"));
    assert!(migration.contains("email TEXT NOT NULL UNIQUE"));
    assert!(migration.contains("password_hash TEXT NOT NULL"));
    assert!(migration.contains("user_id INTEGER NOT NULL REFERENCES users(id)"));
    assert!(
        migration.contains("UNIQUE INDEX IF NOT EXISTS idx_document_interactions_pending_actor")
    );
}

#[test]
fn api_key_migration_contains_public_api_key_contracts() {
    let migration = include_str!("../migrations/003_api_keys.sql");

    assert!(migration.contains("CREATE TABLE IF NOT EXISTS api_keys"));
    assert!(migration.contains("token TEXT NOT NULL UNIQUE"));
    assert!(migration.contains("key_hash TEXT NOT NULL UNIQUE"));
    assert!(migration.contains("revoked_at TEXT"));
    assert!(migration.contains("hidden_at TEXT"));
    assert!(migration.contains("created_by_user_id INTEGER NOT NULL REFERENCES users(id)"));
}

#[test]
fn mcp_key_migration_contains_mcp_key_contracts() {
    let migration = include_str!("../migrations/004_mcp_keys.sql");

    assert!(migration.contains("CREATE TABLE IF NOT EXISTS mcp_keys"));
    assert!(migration.contains("token TEXT NOT NULL UNIQUE"));
    assert!(migration.contains("key_hash TEXT NOT NULL UNIQUE"));
    assert!(migration.contains("capabilities TEXT NOT NULL DEFAULT"));
    assert!(migration.contains("documents:read"));
    assert!(migration.contains("documents:explain"));
    assert!(migration.contains("revoked_at TEXT"));
    assert!(migration.contains("hidden_at TEXT"));
    assert!(migration.contains("created_by_user_id INTEGER NOT NULL REFERENCES users(id)"));
}

#[test]
fn accounting_requested_migration_adds_legacy_documents_column() {
    let migration = include_str!("../migrations/005_accounting_requested.sql");

    assert!(migration.contains("ALTER TABLE documents"));
    assert!(migration.contains("ADD COLUMN accounting_requested_at TEXT"));
    assert!(migration.contains("idx_documents_accounting_requested"));
}

#[test]
fn document_state_migration_creates_domain_state_tables() {
    let migration = include_str!("../migrations/006_document_state.sql");

    assert!(migration.contains("CREATE TABLE IF NOT EXISTS document_intake_state"));
    assert!(migration.contains("CREATE TABLE IF NOT EXISTS document_accounting_state"));
    assert!(!migration.contains("CREATE TABLE IF NOT EXISTS document_review_state"));
    assert!(!migration.contains("CREATE TABLE IF NOT EXISTS document_export_state"));
    assert!(migration.contains("trg_documents_document_state_after_insert"));
    assert!(migration.contains("trg_documents_document_state_after_update"));
}

#[test]
fn document_state_cleanup_migration_removes_legacy_state_columns() {
    let migration = include_str!("../migrations/007_finalize_document_state.sql");

    assert!(migration.contains("DROP TRIGGER IF EXISTS trg_documents_document_state_after_insert"));
    assert!(migration.contains("DROP TRIGGER IF EXISTS trg_documents_document_state_after_update"));
    assert!(migration.contains("DROP INDEX IF EXISTS idx_documents_status"));
    assert!(migration.contains("ALTER TABLE documents DROP COLUMN status"));
    assert!(migration.contains("ALTER TABLE documents DROP COLUMN vision_started_at"));
    assert!(migration.contains("ALTER TABLE documents DROP COLUMN vision_completed_at"));
    assert!(migration.contains("ALTER TABLE documents DROP COLUMN accountant_reviewed_at"));
    assert!(migration.contains("ALTER TABLE documents DROP COLUMN validated_at"));
    assert!(migration.contains("ALTER TABLE documents DROP COLUMN review_completed_at"));
    assert!(migration.contains("ALTER TABLE documents DROP COLUMN exported_at"));
    assert!(migration.contains("ALTER TABLE documents DROP COLUMN exported_in_batch"));
    assert!(migration.contains("ALTER TABLE documents DROP COLUMN accounting_requested_at"));
    assert!(!migration.contains("CREATE VIEW document_state AS"));
}

#[test]
fn document_state_run_migration_creates_run_tables_without_a_view() {
    let migration = include_str!("../migrations/008_document_state_runs.sql");

    assert!(migration.contains("CREATE TABLE IF NOT EXISTS document_intake_runs"));
    assert!(migration.contains("CREATE TABLE IF NOT EXISTS document_accounting_runs"));
    assert!(!migration.contains("CREATE TABLE IF NOT EXISTS document_review_runs"));
    assert!(!migration.contains("CREATE TABLE IF NOT EXISTS document_export_runs"));
    assert!(!migration.contains("CREATE VIEW document_state AS"));
}

#[tokio::test]
async fn embedded_sqlx_migrations_create_application_schema() {
    let pool = common::in_memory_pool().await;

    let documents_exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'documents'",
    )
    .fetch_one(&pool)
    .await
    .expect("documents lookup");
    assert_eq!(documents_exists, 1);

    let intake_state_exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'document_intake_state'",
    )
    .fetch_one(&pool)
    .await
    .expect("document_intake_state lookup");
    assert_eq!(intake_state_exists, 1);

    let status_column_exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('documents') WHERE name = 'status'",
    )
    .fetch_one(&pool)
    .await
    .expect("documents.status lookup");
    assert_eq!(status_column_exists, 0);

    let accounting_requested_column_exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('documents') WHERE name = 'accounting_requested_at'",
    )
    .fetch_one(&pool)
    .await
    .expect("documents.accounting_requested_at lookup");
    assert_eq!(accounting_requested_column_exists, 0);

    let state_view_exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'view' AND name = 'document_state'",
    )
    .fetch_one(&pool)
    .await
    .expect("document_state lookup");
    assert_eq!(state_view_exists, 0);

    let intake_runs_exists: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'document_intake_runs'",
    )
    .fetch_one(&pool)
    .await
    .expect("document_intake_runs lookup");
    assert_eq!(intake_runs_exists, 1);

    let migration_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("sqlx migration metadata");
    assert!(migration_rows >= 1);
}
