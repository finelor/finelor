use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;

use crate::error::AppResult;

#[derive(Clone, Default)]
pub struct EphemeralStore {
    entries: Arc<Mutex<HashMap<String, Entry>>>,
}

#[derive(Clone)]
struct Entry {
    value: String,
    expires_at: Option<i64>,
}

impl EphemeralStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn set(&self, key: &str, value: &str, ttl_seconds: Option<u64>) -> AppResult<()> {
        let expires_at = ttl_seconds.map(|ttl| Utc::now().timestamp() + ttl as i64);
        let mut entries = self.entries.lock().expect("ephemeral store mutex poisoned");
        entries.insert(
            key.to_string(),
            Entry {
                value: value.to_string(),
                expires_at,
            },
        );
        Ok(())
    }

    pub async fn get(&self, key: &str) -> AppResult<Option<String>> {
        let mut entries = self.entries.lock().expect("ephemeral store mutex poisoned");
        delete_expired_key(&mut entries, key);
        Ok(entries.get(key).map(|entry| entry.value.clone()))
    }

    pub async fn consume(&self, key: &str) -> AppResult<Option<String>> {
        let mut entries = self.entries.lock().expect("ephemeral store mutex poisoned");
        delete_expired_key(&mut entries, key);
        Ok(entries.remove(key).map(|entry| entry.value))
    }

    pub async fn delete(&self, key: &str) -> AppResult<bool> {
        let mut entries = self.entries.lock().expect("ephemeral store mutex poisoned");
        Ok(entries.remove(key).is_some())
    }

    pub async fn claim(&self, key: &str, value: &str, ttl_seconds: u64) -> AppResult<bool> {
        let mut entries = self.entries.lock().expect("ephemeral store mutex poisoned");
        delete_expired_key(&mut entries, key);
        if entries.contains_key(key) {
            return Ok(false);
        }
        entries.insert(
            key.to_string(),
            Entry {
                value: value.to_string(),
                expires_at: Some(Utc::now().timestamp() + ttl_seconds as i64),
            },
        );
        Ok(true)
    }

    pub async fn cleanup_expired(&self) -> AppResult<u64> {
        let mut entries = self.entries.lock().expect("ephemeral store mutex poisoned");
        let before = entries.len();
        delete_expired(&mut entries);
        Ok((before - entries.len()) as u64)
    }
}

fn delete_expired_key(entries: &mut HashMap<String, Entry>, key: &str) {
    if entries.get(key).is_some_and(is_expired) {
        entries.remove(key);
    }
}

fn delete_expired(entries: &mut HashMap<String, Entry>) {
    entries.retain(|_, entry| !is_expired(entry));
}

fn is_expired(entry: &Entry) -> bool {
    entry
        .expires_at
        .is_some_and(|expires_at| expires_at <= Utc::now().timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> EphemeralStore {
        EphemeralStore::new()
    }

    #[tokio::test]
    async fn set_get_and_delete_value() {
        let store = store();
        store.set("a", "b", Some(60)).await.unwrap();
        assert_eq!(store.get("a").await.unwrap(), Some("b".to_string()));
        assert!(store.delete("a").await.unwrap());
        assert_eq!(store.get("a").await.unwrap(), None);
    }

    #[tokio::test]
    async fn consume_returns_value_once() {
        let store = store();
        store.set("a", "b", Some(60)).await.unwrap();
        assert_eq!(store.consume("a").await.unwrap(), Some("b".to_string()));
        assert_eq!(store.consume("a").await.unwrap(), None);
    }

    #[tokio::test]
    async fn expired_value_is_not_returned() {
        let store = store();
        store.set("a", "b", Some(0)).await.unwrap();
        assert_eq!(store.get("a").await.unwrap(), None);
    }

    #[tokio::test]
    async fn claim_succeeds_once_until_release() {
        let store = store();
        assert!(store.claim("lock", "processing", 60).await.unwrap());
        assert!(!store.claim("lock", "processing", 60).await.unwrap());
        assert!(store.delete("lock").await.unwrap());
        assert!(store.claim("lock", "processing", 60).await.unwrap());
    }

    #[tokio::test]
    async fn cleanup_expired_removes_only_expired_values() {
        let store = store();
        store.set("expired", "a", Some(0)).await.unwrap();
        store.set("live", "b", Some(60)).await.unwrap();
        store.set("persistent", "c", None).await.unwrap();

        assert_eq!(store.cleanup_expired().await.unwrap(), 1);
        assert_eq!(store.get("expired").await.unwrap(), None);
        assert_eq!(store.get("live").await.unwrap(), Some("b".to_string()));
        assert_eq!(
            store.get("persistent").await.unwrap(),
            Some("c".to_string())
        );
    }

    #[tokio::test]
    async fn claim_succeeds_after_existing_claim_expires() {
        let store = store();
        assert!(store.claim("lock", "first", 0).await.unwrap());
        assert!(store.claim("lock", "second", 60).await.unwrap());
        assert_eq!(store.get("lock").await.unwrap(), Some("second".to_string()));
    }
}
