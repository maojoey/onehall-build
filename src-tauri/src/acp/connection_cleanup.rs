//! One process-wide cleanup worker, independent of every Tokio runtime.
//! A queued job retains its ID on scheduling failure and compares connection
//! incarnation before removal, so an old driver's Drop cannot delete a replacement.
use super::connection::AgentConnection;
use std::{
    collections::HashMap,
    sync::{atomic::AtomicU32, Arc, Mutex, OnceLock, Weak},
};
use tokio::sync::{oneshot, Mutex as AsyncMutex};

type Connections = AsyncMutex<HashMap<String, AgentConnection>>;
#[derive(Clone)]
struct Removal {
    connections: Weak<Connections>,
    id: String,
    incarnation: Arc<AtomicU32>,
}
impl Removal {
    fn remove(&self, map: &mut HashMap<String, AgentConnection>) -> Option<AgentConnection> {
        if map
            .get(&self.id)
            .is_some_and(|c| Arc::ptr_eq(&c.child_pid, &self.incarnation))
        {
            map.remove(&self.id)
        } else {
            None
        }
    }
    fn run(&self) {
        if let Some(map) = self.connections.upgrade() {
            let removed = self.remove(&mut map.blocking_lock());
            drop(removed);
        }
    }
}
enum Job {
    Remove(Removal),
    Barrier(oneshot::Sender<()>),
}
#[derive(Default)]
struct State {
    sender: Option<std::sync::mpsc::Sender<Job>>,
    pending: Vec<Job>,
}
#[derive(Default)]
struct Queue(Mutex<State>);
impl Queue {
    fn global() -> &'static Self {
        static QUEUE: OnceLock<Queue> = OnceLock::new();
        QUEUE.get_or_init(Self::default)
    }
    fn submit(&self, job: Job) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        state.pending.push(job);
        if state.sender.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<Job>();
            let spawned = std::thread::Builder::new()
                .name("acp-cleanup".into())
                .spawn(move || {
                    for job in rx {
                        match job {
                            Job::Remove(removal) => {
                                // Keep an owned copy available if a future connection Drop panics.
                                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    removal.run()
                                }))
                                .is_err()
                                {
                                    tracing::error!("[ACP] cleanup_failed class=worker_unwind");
                                    Queue::global().submit(Job::Remove(removal));
                                }
                            }
                            Job::Barrier(done) => {
                                let _ = done.send(());
                            }
                        }
                    }
                });
            if spawned.is_err() {
                tracing::error!(
                    "[ACP] cleanup_failed class=worker_unavailable pending_retained=true"
                );
                return false;
            }
            state.sender = Some(tx);
        }
        let pending = std::mem::take(&mut state.pending);
        let mut iter = pending.into_iter();
        while let Some(job) = iter.next() {
            if let Err(err) = state.sender.as_ref().expect("worker sender").send(job) {
                state.pending.push(err.0);
                state.pending.extend(iter);
                state.sender = None;
                tracing::error!("[ACP] cleanup_failed class=worker_closed pending_retained=true");
                return false;
            }
        }
        true
    }
}
pub(super) struct ConnectionCleanupGuard(Removal);
impl ConnectionCleanupGuard {
    pub(super) fn new(
        connections: &Arc<Connections>,
        id: String,
        incarnation: Arc<AtomicU32>,
    ) -> Self {
        Self(Removal {
            connections: Arc::downgrade(connections),
            id,
            incarnation,
        })
    }
}
impl Drop for ConnectionCleanupGuard {
    fn drop(&mut self) {
        let Some(map) = self.0.connections.upgrade() else {
            return;
        };
        if let Ok(mut held) = map.try_lock() {
            let removed = self.0.remove(&mut held);
            drop(held);
            drop(removed);
            return;
        }
        Queue::global().submit(Job::Remove(self.0.clone()));
    }
}
/// Called after releasing the connection map and signalling every driver.
/// Never wait on this while holding a map lock or from the cleanup worker.
pub(super) async fn drain() -> Result<(), &'static str> {
    let (tx, rx) = oneshot::channel();
    if !Queue::global().submit(Job::Barrier(tx)) {
        return Err("worker_unavailable");
    }
    rx.await.map_err(|_| "worker_closed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        acp::manager::ConnectionManager, models::agent::AgentType, web::event_bridge::EventEmitter,
    };

    async fn fixture() -> (Arc<Connections>, Arc<AtomicU32>) {
        let manager = ConnectionManager::new();
        manager
            .insert_test_connection("fixture", AgentType::Codex, None, EventEmitter::Noop)
            .await;
        let incarnation = manager.connections.lock().await["fixture"]
            .child_pid
            .clone();
        (manager.connections.clone(), incarnation)
    }
    #[test]
    fn cleanup_after_original_runtime_shutdown_removes_real_entry() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (map, incarnation) = runtime.block_on(fixture());
        let held = runtime.block_on(map.clone().lock_owned());
        let guard = ConnectionCleanupGuard::new(&map, "fixture".into(), incarnation);
        drop(runtime);
        assert!(std::thread::spawn(move || drop(guard)).join().is_ok());
        drop(held);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if map.try_lock().is_ok_and(|m| m.is_empty()) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "independent worker must clean up without any Tokio runtime"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    #[tokio::test]
    async fn cleanup_thread_panic_and_delayed_lock_release_are_drained() {
        let (map, incarnation) = fixture().await;
        let held = map.clone().lock_owned().await;
        let guard = ConnectionCleanupGuard::new(&map, "fixture".into(), incarnation);
        assert!(std::thread::spawn(move || {
            let _guard = guard;
            panic!("fixture driver unwind");
        })
        .join()
        .is_err());
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            drop(held);
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), drain())
            .await
            .unwrap()
            .unwrap();
        assert!(map.lock().await.is_empty());
    }
    #[tokio::test]
    async fn cleanup_duplicate_removal_is_idempotent() {
        let (map, incarnation) = fixture().await;
        let a = ConnectionCleanupGuard::new(&map, "fixture".into(), incarnation.clone());
        let b = ConnectionCleanupGuard::new(&map, "fixture".into(), incarnation);
        drop(a);
        drop(b);
        assert!(map.lock().await.is_empty());
    }
    #[tokio::test]
    async fn cleanup_failed_schedule_retains_and_retries_the_connection_id() {
        let (map, incarnation) = fixture().await;
        let queue = Queue::default();
        let (tx, rx) = std::sync::mpsc::channel();
        drop(rx);
        queue.0.lock().unwrap().sender = Some(tx);
        assert!(!queue.submit(Job::Remove(Removal {
            connections: Arc::downgrade(&map),
            id: "fixture".into(),
            incarnation
        })));
        assert_eq!(queue.0.lock().unwrap().pending.len(), 1);
        let (done, wait) = oneshot::channel();
        assert!(queue.submit(Job::Barrier(done)));
        tokio::time::timeout(std::time::Duration::from_secs(2), wait)
            .await
            .unwrap()
            .unwrap();
        assert!(map.lock().await.is_empty());
    }
    #[tokio::test]
    async fn cleanup_old_queued_guard_does_not_remove_new_incarnation() {
        let manager = ConnectionManager::new();
        manager
            .insert_test_connection("fixture", AgentType::Codex, None, EventEmitter::Noop)
            .await;
        let mut held = manager.connections.lock().await;
        let old = held.remove("fixture").unwrap();
        let token = old.child_pid.clone();
        let guard = ConnectionCleanupGuard::new(&manager.connections, "fixture".into(), token);
        let mut replacement = old;
        replacement.child_pid = Arc::new(AtomicU32::new(0));
        held.insert("fixture".into(), replacement);
        drop(guard);
        drop(held);
        tokio::time::timeout(std::time::Duration::from_secs(2), drain())
            .await
            .unwrap()
            .unwrap();
        assert!(manager.connections.lock().await.contains_key("fixture"));
    }
}
