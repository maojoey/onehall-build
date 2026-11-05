//! Short-lived, one-use HQ capabilities. Upstream URLs are never accepted.
use crate::app_error::AppCommandError;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

pub struct TicketStore {
    entries: Mutex<HashMap<String, Ticket>>,
    ttl: Duration,
    capacity: usize,
}
pub struct Ticket {
    pub branch_id: String,
    pub revision: i64,
    pub upstream: String,
    expires: Instant,
}
impl TicketStore {
    pub fn new(ttl: Duration, capacity: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            capacity,
        }
    }
    pub async fn mint(
        &self,
        branch: &str,
        revision: i64,
        value: &Value,
    ) -> Result<Value, AppCommandError> {
        let invalid = || AppCommandError::network("Branch returned an invalid download ticket");
        let upstream = value
            .get("ticket")
            .and_then(Value::as_str)
            .filter(|s| s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(invalid)?;
        let filename = value
            .get("filename")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 1024)
            .ok_or_else(invalid)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let remote_expiry = value
            .get("expiresAt")
            .and_then(Value::as_i64)
            .filter(|e| *e > now)
            .ok_or_else(invalid)?;
        let ttl = self
            .ttl
            .min(Duration::from_secs(remote_expiry.saturating_sub(now) as u64));
        let mut entries = self.entries.lock().await;
        entries.retain(|_, t| Instant::now() < t.expires);
        if entries.len() >= self.capacity {
            return Err(AppCommandError::network(
                "Branch download queue is full; retry shortly",
            ));
        }
        let cap = uuid::Uuid::new_v4().simple().to_string();
        entries.insert(
            cap.clone(),
            Ticket {
                branch_id: branch.into(),
                revision,
                upstream: upstream.into(),
                expires: Instant::now() + ttl,
            },
        );
        Ok(
            json!({"ticket":cap,"url":format!("/branch-download/{cap}"),"filename":filename,"expiresAt":now+ttl.as_secs() as i64}),
        )
    }
    pub async fn take(&self, cap: &str, branch: Option<&str>) -> Option<Ticket> {
        let mut entries = self.entries.lock().await;
        entries.retain(|_, t| Instant::now() < t.expires);
        let entry = entries.get(cap)?;
        if branch.is_some_and(|b| b != entry.branch_id) {
            return None;
        }
        entries.remove(cap)
    }
}
