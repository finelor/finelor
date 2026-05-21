use crate::db::DbPool;
use crate::kv::EphemeralStore;
use std::sync::OnceLock;

static POOL: OnceLock<DbPool> = OnceLock::new();
static EPHEMERAL_STORE: OnceLock<EphemeralStore> = OnceLock::new();

/// Must be called exactly once at startup before any server function runs.
pub fn set_pool(pool: DbPool) {
    let _ = POOL.set(pool);
}

pub fn set_ephemeral_store(store: EphemeralStore) {
    let _ = EPHEMERAL_STORE.set(store);
}

/// Panics if `set_pool` has not been called.
pub fn get_pool() -> DbPool {
    POOL.get()
        .expect("DbPool not initialized: call web::pool::set_pool() before serving requests")
        .clone()
}

pub fn get_ephemeral_store() -> EphemeralStore {
    EPHEMERAL_STORE
        .get()
        .expect("EphemeralStore not initialized: call web::pool::set_ephemeral_store() before serving requests")
        .clone()
}
