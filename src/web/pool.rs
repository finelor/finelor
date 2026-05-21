use crate::db::DbPool;
use crate::kv::EphemeralStore;
use std::sync::OnceLock;

static POOL: OnceLock<DbPool> = OnceLock::new();
static EPHEMERAL_STORE: OnceLock<EphemeralStore> = OnceLock::new();

static TEST_POOL_OVERRIDE_LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
static TEST_POOL_OVERRIDE: OnceLock<std::sync::Mutex<Option<DbPool>>> = OnceLock::new();

/// Must be called exactly once at startup before any server function runs.
pub fn set_pool(pool: DbPool) {
    let _ = POOL.set(pool);
}

pub fn set_ephemeral_store(store: EphemeralStore) {
    let _ = EPHEMERAL_STORE.set(store);
}

/// Panics if `set_pool` has not been called.
pub fn get_pool() -> DbPool {
    if let Some(override_cell) = TEST_POOL_OVERRIDE.get() {
        let guard = override_cell
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(pool) = guard.as_ref() {
            return pool.clone();
        }
    }

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

pub struct TestPoolOverrideGuard {
    _lock_guard: std::sync::MutexGuard<'static, ()>,
    previous: Option<DbPool>,
}

impl Drop for TestPoolOverrideGuard {
    fn drop(&mut self) {
        let override_cell = TEST_POOL_OVERRIDE
            .get()
            .expect("test pool override cell should be initialized");
        let mut guard = override_cell
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = self.previous.take();
    }
}

pub async fn override_pool_for_test(pool: DbPool) -> TestPoolOverrideGuard {
    let lock = TEST_POOL_OVERRIDE_LOCK.get_or_init(|| std::sync::Mutex::new(()));
    let lock_guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

    let override_cell = TEST_POOL_OVERRIDE.get_or_init(|| std::sync::Mutex::new(None));
    let mut override_guard = override_cell
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = override_guard.replace(pool);
    drop(override_guard);

    TestPoolOverrideGuard {
        _lock_guard: lock_guard,
        previous,
    }
}
