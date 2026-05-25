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
    let migration = include_str!("../migrations/002_api_keys.sql");

    assert!(migration.contains("CREATE TABLE IF NOT EXISTS api_keys"));
    assert!(migration.contains("token TEXT NOT NULL UNIQUE"));
    assert!(migration.contains("key_hash TEXT NOT NULL UNIQUE"));
    assert!(migration.contains("revoked_at TEXT"));
    assert!(migration.contains("hidden_at TEXT"));
    assert!(migration.contains("created_by_user_id INTEGER NOT NULL REFERENCES users(id)"));
}

#[test]
fn mcp_key_migration_contains_mcp_key_contracts() {
    let migration = include_str!("../migrations/003_mcp_keys.sql");

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

    let migration_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("sqlx migration metadata");
    assert!(migration_rows >= 1);
}
