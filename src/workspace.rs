use crate::db::DbPool;
use crate::error::AppResult;
use uuid::Uuid;

pub const DEFAULT_WORKSPACE_ID: &str = "00000000-0000-0000-0000-000000000001";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceContext {
    pub id: Uuid,
}

pub trait WorkspaceResolver: Send + Sync {
    fn active_workspace(&self) -> WorkspaceContext;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SingleWorkspaceResolver;

impl WorkspaceResolver for SingleWorkspaceResolver {
    fn active_workspace(&self) -> WorkspaceContext {
        WorkspaceContext {
            id: active_workspace_id(),
        }
    }
}

pub fn active_workspace_id() -> Uuid {
    Uuid::parse_str(DEFAULT_WORKSPACE_ID).expect("valid workspace id")
}

pub async fn ensure_single_workspace(pool: &DbPool) -> AppResult<()> {
    sqlx::query("SELECT 1").execute(pool).await?;
    Ok(())
}
