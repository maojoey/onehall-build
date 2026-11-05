use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Weak,
};
use std::time::Duration;

use futures::{Sink, SinkExt, Stream, StreamExt};

use axum::extract::ws::{Message, WebSocket};
use axum::{
    extract::{Extension, WebSocketUpgrade},
    response::IntoResponse,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::event_bridge::WebEventBroadcaster;
use super::shutdown::ShutdownSignal;
use super::ws_attach::{self, ClientMsg, DetachReason, ServerMsg, OUTBOUND_CAPACITY};
use crate::acp::{internal_bus::EventBusMetrics, manager::ConnectionManager};
use crate::app_state::AppState;
use crate::logging::throttle::{LagLogThrottle, LAG_LOG_WINDOW};

// Match the remote transports: a blocked write must release the session.
const SOCKET_SEND_TIMEOUT: Duration = Duration::from_secs(8);

async fn send_socket_message<S>(socket: &mut S, message: Message) -> bool
where
    S: Sink<Message> + Unpin,
{
    let bytes = match &message {
        Message::Text(text) => text.len(),
        Message::Binary(bytes) | Message::Ping(bytes) | Message::Pong(bytes) => bytes.len(),
        Message::Close(frame) => frame.as_ref().map_or(0, |frame| frame.reason.len()),
    };
    let started = tokio::time::Instant::now();
    let reason = match tokio::time::timeout(SOCKET_SEND_TIMEOUT, socket.send(message)).await {
        Ok(Ok(())) => return true,
        Ok(Err(_)) => "send_failed",
        Err(_) => "send_timeout",
    };
    tracing::warn!(
        side = "client",
        reason,
        bytes,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "event socket write stopped"
    );
    false
}

/// One entry per live attach subscription. The `epoch` is the per-WS-session
/// monotonic counter assigned at spawn time; it threads through the cleanup
/// channel so a stale exit signal from a now-replaced handle cannot
/// accidentally remove the fresh handle. See the `cleanup_tx` comment in
/// `run_ws_connection` for full rationale.
struct ActiveSubscription {
    handle: JoinHandle<()>,
    epoch: u64,
}

impl Drop for ActiveSubscription {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Weak generation entries remain while queued frames own their token. A
/// replacement can therefore invalidate old frames even after their forwarder
/// exited and its cleanup signal removed the active handle.
#[derive(Default)]
struct Subscriptions {
    active: HashMap<String, ActiveSubscription>,
    generations: HashMap<String, Weak<AtomicBool>>,
}

impl Subscriptions {
    fn invalidate(&mut self, id: &str) {
        if let Some(current) = self.generations.remove(id).and_then(|v| v.upgrade()) {
            current.store(false, Ordering::Relaxed);
        }
        self.generations.retain(|_, token| token.strong_count() > 0);
        self.active.remove(id);
    }

    fn begin_generation(&mut self, id: &str) -> Arc<AtomicBool> {
        self.invalidate(id);
        let current = Arc::new(AtomicBool::new(true));
        self.generations
            .insert(id.to_owned(), Arc::downgrade(&current));
        current
    }
}

/// Apply a forwarder self-cleanup signal: remove the handle for `sub_id`
/// only if the stored epoch matches `signal_epoch`. Extracted from the
/// select! branch so the epoch-matching invariant has a unit test.
fn apply_cleanup_signal(
    subscriptions: &mut HashMap<String, ActiveSubscription>,
    sub_id: &str,
    signal_epoch: u64,
) {
    if let Some(sub) = subscriptions.get(sub_id) {
        if sub.epoch == signal_epoch {
            subscriptions.remove(sub_id);
        }
    }
}

// MUST match `WS_READY_CHANNEL` in `src/lib/transport/constants.ts`.
// Drift between the two values silently breaks the handshake (the client
// keeps waiting and falls back to the timeout warning path after 5 s).
//
// Phase 1: kept active in parallel with the new attach protocol so existing
// clients (web / remote desktop) continue to work while transports migrate.
// Phase 4 will retire this channel.
const WS_READY_CHANNEL: &str = "__ready__";

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Extension(state): Extension<Arc<AppState>>,
    Extension(shutdown_signal): Extension<Arc<ShutdownSignal>>,
) -> impl IntoResponse {
    ws.protocols([super::auth::WS_EVENT_PROTOCOL])
        .on_upgrade(|socket| handle_ws_connection(socket, state, shutdown_signal))
}

async fn handle_ws_connection(
    socket: WebSocket,
    state: Arc<AppState>,
    shutdown_signal: Arc<ShutdownSignal>,
) {
    run_ws_connection(
        socket,
        &state.connection_manager,
        state.acp_event_bus.metrics(),
        &state.event_broadcaster,
        shutdown_signal,
    )
    .await;
}

async fn run_ws_connection<S, E>(
    mut socket: S,
    manager: &ConnectionManager,
    metrics: &Arc<EventBusMetrics>,
    event_broadcaster: &WebEventBroadcaster,
    shutdown_signal: Arc<ShutdownSignal>,
) where
    S: Sink<Message> + Stream<Item = Result<Message, E>> + Unpin,
{
    // Late handshake guard: if shutdown already fired before this task
    // even started, exit before subscribing to anything else.
    if shutdown_signal.is_triggered() {
        let _ = send_socket_message(&mut socket, Message::Close(None)).await;
        return;
    }

    // Legacy global firehose subscriber. Removed in Phase 4 once all
    // transports use the attach protocol.
    let mut global_rx = event_broadcaster.subscribe();

    // Outbound channel funnels every server-→-client frame through one
    // sender so the WS write side has a single owner. Per-attach forwarder
    // tasks push `Event`/`Detached` frames here; the main loop pushes
    // `Snapshot`/`Replay`/`Pong` directly.
    let (outbound_tx, mut outbound_rx) =
        mpsc::channel::<ws_attach::ForwardedMsg>(OUTBOUND_CAPACITY);

    // Cleanup channel: forwarders signal `(subscription_id, epoch)` here
    // when they self-exit (lagged / channel closed) so the main loop drops
    // the now-completed `JoinHandle` from `subscriptions`. Without this,
    // dead handles would sit in the map until socket close. Sized at
    // OUTBOUND_CAPACITY which comfortably covers a burst of simultaneous
    // forwarder exits; on overflow the forwarder's `try_send` no-ops and
    // the socket-close drain still reaps everything.
    //
    // The epoch is what makes the cleanup race-free. `JoinHandle::is_finished()`
    // looked tempting but is racy on multi-threaded Tokio — the cleanup
    // signal can land before the runtime has marked the JoinHandle's slot
    // finished. Instead, each Attach allocates a fresh u64 epoch (monotonic
    // per WS session) and stores it alongside the handle. The cleanup
    // branch only removes when the stored epoch matches the signal's
    // epoch — re-attaching with the same subscription_id stamps a new
    // epoch so any stale signal from the previous handle becomes a no-op.
    let (cleanup_tx, mut cleanup_rx) = mpsc::channel::<(String, u64)>(OUTBOUND_CAPACITY);

    // Track active attach subscriptions on this socket so a `detach`
    // message can abort the matching forwarder task and so we can clean
    // them all up on socket close. Each entry stores the forwarder's
    // epoch (see cleanup channel above) alongside the JoinHandle.
    let mut subscriptions = Subscriptions::default();
    let mut next_epoch: u64 = 0;

    // Server→client ready handshake (legacy `__ready__` frame). Phase 1
    // keeps this so unmigrated transports still gate `acp_connect` on the
    // server-side receiver being subscribed. New attach-protocol clients
    // ignore this frame (channel name doesn't match any attach payload).
    let ready_payload = serde_json::json!({
        "channel": WS_READY_CHANNEL,
        "payload": null,
    });
    match serde_json::to_string(&ready_payload) {
        Ok(text) => {
            if !send_socket_message(&mut socket, Message::Text(text.into())).await {
                tracing::warn!("[WS][WARN] ready send failed or timed out");
                return;
            }
        }
        Err(e) => {
            tracing::warn!("[WS][WARN] failed to serialize __ready__ frame: {e}");
            return;
        }
    }

    // Throttles the per-connection global-firehose lag WARN so a slow client
    // reading behind a steady side-channel stream can't trickle near-duplicate
    // lines. Task-local: one instance per WS connection.
    let mut lag_throttle = LagLogThrottle::new(LAG_LOG_WINDOW);

    loop {
        tokio::select! {
            // Server-initiated shutdown: notify any active attach
            // subscriptions before closing so the client can decide
            // whether to retry on the next reconnect.
            _ = shutdown_signal.wait() => {
                for sub_id in subscriptions.active.keys() {
                    let frame = ServerMsg::Detached {
                        subscription_id: sub_id.clone(),
                        reason: DetachReason::ServerShutdown,
                    };
                    if let Ok(text) = serde_json::to_string(&frame) {
                        if !send_socket_message(&mut socket, Message::Text(text.into())).await {
                            break;
                        }
                    }
                }
                let _ = send_socket_message(&mut socket, Message::Close(None)).await;
                break;
            }

            // Forwarder self-cleanup: a per-attach task that exited (Lagged
            // or broadcast Closed) reports `(sub_id, epoch)` so we can
            // drop the dead JoinHandle. The epoch match guarantees we
            // only remove the handle the signal was actually for —
            // a re-attach between the dead forwarder's exit and this
            // recv would have stamped a new epoch, making the stale
            // signal a no-op.
            cleanup = cleanup_rx.recv() => {
                if let Some((sub_id, signal_epoch)) = cleanup {
                    apply_cleanup_signal(&mut subscriptions.active, &sub_id, signal_epoch);
                }
            }

            // Bounded queue for per-attach forwarder frames.
            outgoing = outbound_rx.recv() => {
                match outgoing {
                    Some(queued) => {
                        // Re-attach invalidates the generation, so queued old events
                        // cannot appear after its new initial snapshot.
                        if !queued.current.load(Ordering::Relaxed) {
                            continue;
                        }
                        match serde_json::to_string(&queued.message) {
                            Ok(text) => {
                                if !send_socket_message(&mut socket, Message::Text(text.into())).await {
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::warn!("[WS][WARN] failed to serialize ServerMsg: {e}");
                            }
                        }
                    }
                    // Channel closed only when all senders dropped — i.e. this
                    // task itself dropped `outbound_tx` AND every spawned
                    // forwarder exited. Won't happen while the loop runs.
                    None => break,
                }
            }

            // Legacy global firehose. Forwarded as-is (uses the old
            // `WebEvent { channel, payload }` shape, not the attach
            // protocol's `ServerMsg`).
            result = global_rx.recv() => {
                match result {
                    Ok(event) => {
                        if let Ok(msg) = serde_json::to_string(&event) {
                            if !send_socket_message(&mut socket, Message::Text(msg.into())).await {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        if let Some(s) = lag_throttle.record(n) {
                            tracing::warn!(
                                "[WS][WARN] global receiver lagged: skipped {} events across \
                                 {} occurrence(s) in the last {}s",
                                s.dropped,
                                s.occurrences,
                                LAG_LOG_WINDOW.as_secs()
                            );
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }

            // Client-→-server messages. Text frames are parsed as
            // `ClientMsg`; everything else is ignored (binary, ping/pong
            // are handled by axum, close is handled by the None match arm).
            msg = socket.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMsg>(&text) {
                            Ok(cmsg) => {
                                let reply = handle_client_msg(
                                    cmsg,
                                    manager,
                                    metrics,
                                    &outbound_tx,
                                    &cleanup_tx,
                                    &mut subscriptions,
                                    &mut next_epoch,
                                ).await;
                                // Direct replies never enter the queue this
                                // loop alone drains, even when it is full.
                                if let Some(reply) = reply {
                                    if let Ok(text) = serde_json::to_string(&reply) {
                                        if !send_socket_message(&mut socket, Message::Text(text.into())).await {
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(_) => {
                                tracing::warn!("[WS][WARN] malformed client message");
                            }
                        }
                    }
                    Some(Ok(_)) => {
                        // Binary / ping / pong: ignore.
                    }
                    _ => break,
                }
            }
        }
    }

    // Cleanup: abort all active forwarder tasks. Their broadcast receivers
    // will be dropped, freeing the per-connection broadcaster slot.
    for (_, sub) in subscriptions.active.drain() {
        sub.handle.abort();
    }
}

async fn handle_client_msg(
    msg: ClientMsg,
    manager: &ConnectionManager,
    metrics: &Arc<EventBusMetrics>,
    outbound_tx: &mpsc::Sender<ws_attach::ForwardedMsg>,
    cleanup_tx: &mpsc::Sender<(String, u64)>,
    subscriptions: &mut Subscriptions,
    next_epoch: &mut u64,
) -> Option<ServerMsg> {
    match msg {
        ClientMsg::Attach {
            subscription_id,
            connection_id,
            since_seq,
        } => {
            // Re-attach with the same subscription_id replaces the prior
            // forwarder. Abort the old one first so its receiver drops
            // and we don't leak a broadcaster slot.
            let current = subscriptions.begin_generation(&subscription_id);

            match ws_attach::handle_attach(
                manager,
                metrics,
                subscription_id.clone(),
                connection_id,
                since_seq,
            )
            .await
            {
                Ok(outcome) => {
                    // The caller writes this initial frame before draining
                    // the forwarder queue, preserving snapshot/event order.
                    // Allocate a fresh epoch for this spawn. wrapping_add is
                    // defensive — u64 overflow per WS session is impossible
                    // in practice (would require ~10^19 attaches on one
                    // socket) but the wrap behavior is well-defined and
                    // matches our epoch-equality semantics.
                    *next_epoch = next_epoch.wrapping_add(1);
                    let epoch = *next_epoch;
                    let handle = ws_attach::spawn_forwarder(
                        subscription_id.clone(),
                        epoch,
                        metrics.clone(),
                        outcome.receiver,
                        ws_attach::ForwarderOutput {
                            sender: outbound_tx.clone(),
                            current,
                        },
                        cleanup_tx.clone(),
                    );
                    subscriptions
                        .active
                        .insert(subscription_id, ActiveSubscription { handle, epoch });
                    Some(outcome.initial_msg)
                }
                Err(reason) => Some(ServerMsg::Detached {
                    subscription_id,
                    reason,
                }),
            }
        }
        ClientMsg::Detach { subscription_id } => {
            subscriptions.invalidate(&subscription_id);
            None
        }
        ClientMsg::Ping => Some(ServerMsg::Pong),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an ActiveSubscription wrapping a no-op spawned task. The
    /// JoinHandle is real so abort() in cleanup paths is realistic.
    async fn dummy_sub(epoch: u64) -> ActiveSubscription {
        let handle = tokio::spawn(async {
            // Park forever; aborted by cleanup paths or dropped at test end.
            std::future::pending::<()>().await;
        });
        ActiveSubscription { handle, epoch }
    }

    #[tokio::test]
    async fn cleanup_signal_with_matching_epoch_removes_entry() {
        let mut subs = HashMap::new();
        subs.insert("sub-A".to_string(), dummy_sub(1).await);

        apply_cleanup_signal(&mut subs, "sub-A", 1);

        assert!(!subs.contains_key("sub-A"));
    }

    #[tokio::test]
    async fn cleanup_signal_with_stale_epoch_keeps_fresh_handle() {
        // Simulate the race: old forwarder exited with epoch=1; client
        // re-attached and a new forwarder was spawned with epoch=2 in the
        // same map slot. The old cleanup signal must NOT remove the fresh
        // handle, otherwise the new forwarder is orphaned.
        let mut subs = HashMap::new();
        let fresh = dummy_sub(2).await;
        subs.insert("sub-A".to_string(), fresh);

        apply_cleanup_signal(&mut subs, "sub-A", 1);

        assert!(
            subs.contains_key("sub-A"),
            "stale cleanup must not evict the freshly re-attached handle"
        );
        assert_eq!(subs.get("sub-A").unwrap().epoch, 2);
    }

    #[tokio::test]
    async fn cleanup_signal_for_unknown_sub_id_is_noop() {
        let mut subs = HashMap::new();
        subs.insert("other".to_string(), dummy_sub(1).await);

        apply_cleanup_signal(&mut subs, "missing", 1);

        assert!(subs.contains_key("other"));
    }
    #[tokio::test]
    async fn full_outbound_queue_does_not_block_ping_or_attach_reply() {
        use crate::{models::AgentType, web::event_bridge::EventEmitter};
        let manager = ConnectionManager::new();
        manager
            .insert_test_connection("live", AgentType::Codex, None, EventEmitter::Noop)
            .await;
        let metrics = Arc::new(EventBusMetrics::default());
        let (tx, _rx) = mpsc::channel(1);
        tx.send(ws_attach::ForwardedMsg {
            current: Arc::new(AtomicBool::new(true)),
            message: ServerMsg::Pong,
        })
        .await
        .unwrap_or_else(|_| panic!("receiver is alive"));
        let (cleanup, _) = mpsc::channel(1);
        let mut subscriptions = Subscriptions::default();
        let mut epoch = 0;
        for (message, kind) in [
            (ClientMsg::Ping, "pong"),
            (
                ClientMsg::Attach {
                    subscription_id: "s".into(),
                    connection_id: "missing".into(),
                    since_seq: None,
                },
                "detached",
            ),
            (
                ClientMsg::Attach {
                    subscription_id: "s".into(),
                    connection_id: "live".into(),
                    since_seq: None,
                },
                "snapshot",
            ),
        ] {
            let reply = tokio::time::timeout(
                Duration::from_millis(100),
                handle_client_msg(
                    message,
                    &manager,
                    &metrics,
                    &tx,
                    &cleanup,
                    &mut subscriptions,
                    &mut epoch,
                ),
            )
            .await
            .expect("direct replies cannot wait for their own outbound consumer")
            .unwrap();
            assert_eq!(serde_json::to_value(reply).unwrap()["type"], kind);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_socket_write_exits_at_deadline() {
        let mut sink = Box::pin(futures::sink::unfold((), |(), _: Message| async {
            std::future::pending::<Result<(), ()>>().await
        }));
        let started = tokio::time::Instant::now();
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(9),
                send_socket_message(&mut sink, Message::Close(None))
            )
            .await,
            Ok(false)
        );
        assert_eq!(started.elapsed(), Duration::from_secs(8));
    }

    #[tokio::test]
    async fn writable_socket_accepts_frame() {
        let mut sink = futures::sink::drain();
        assert!(send_socket_message(&mut sink, Message::Text("ready".into())).await);
    }

    #[tokio::test]
    async fn replacement_invalidates_queued_frames_after_forwarder_cleanup() {
        let mut subscriptions = Subscriptions::default();
        let old = subscriptions.begin_generation("s");
        let terminal = ws_attach::ForwardedMsg {
            current: old.clone(),
            message: ServerMsg::Detached {
                subscription_id: "s".into(),
                reason: DetachReason::ConnectionGone,
            },
        };
        subscriptions.active.insert("s".into(), dummy_sub(1).await);
        apply_cleanup_signal(&mut subscriptions.active, "s", 1);
        assert!(
            terminal.current.load(Ordering::Relaxed),
            "cleanup preserves the queued final Detached"
        );
        let fresh = subscriptions.begin_generation("s");
        assert!(
            !terminal.current.load(Ordering::Relaxed),
            "replacement suppresses the old queued frame"
        );
        assert!(fresh.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn dropping_subscriptions_aborts_forwarders() {
        let mut subscriptions = Subscriptions::default();
        let sub = dummy_sub(1).await;
        let handle = sub.handle.abort_handle();
        subscriptions.active.insert("s".into(), sub);
        drop(subscriptions);
        tokio::task::yield_now().await;
        assert!(handle.is_finished());
    }
    struct StalledInitialReply {
        inbound: Option<Message>,
        sends: usize,
    }

    impl Stream for StalledInitialReply {
        type Item = Result<Message, ()>;
        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            match self.inbound.take() {
                Some(message) => std::task::Poll::Ready(Some(Ok(message))),
                None => std::task::Poll::Pending,
            }
        }
    }

    impl Sink<Message> for StalledInitialReply {
        type Error = ();
        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn start_send(mut self: std::pin::Pin<&mut Self>, _: Message) -> Result<(), Self::Error> {
            self.sends += 1;
            Ok(())
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            if self.sends == 1 {
                std::task::Poll::Ready(Ok(()))
            } else {
                std::task::Poll::Pending
            }
        }
        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn initial_reply_timeout_releases_subscription_receiver() {
        use crate::{models::AgentType, web::event_bridge::EventEmitter};
        let manager = ConnectionManager::new();
        manager
            .insert_test_connection("live", AgentType::Codex, None, EventEmitter::Noop)
            .await;
        let metrics = Arc::new(EventBusMetrics::default());
        let broadcaster = WebEventBroadcaster::new();
        let socket = StalledInitialReply {
            inbound: Some(Message::Text(
                r#"{"action":"attach","subscription_id":"s","connection_id":"live"}"#.into(),
            )),
            sends: 0,
        };
        let result = tokio::time::timeout(
            Duration::from_secs(9),
            run_ws_connection(
                socket,
                &manager,
                &metrics,
                &broadcaster,
                Arc::new(ShutdownSignal::new()),
            ),
        )
        .await;
        assert!(
            result.is_ok(),
            "initial snapshot write must stop at its send deadline"
        );
        tokio::task::yield_now().await;
        let state = manager.get_state("live").await.unwrap();
        let probe = Arc::new(crate::acp::types::EventEnvelope {
            seq: 1,
            connection_id: "live".into(),
            payload: crate::acp::types::AcpEvent::ContentDelta {
                text: "probe".into(),
                parent_tool_use_id: None,
            },
        });
        state.read().await.event_stream().send(probe.clone());
        assert_eq!(
            Arc::strong_count(&probe),
            1,
            "timed-out initial reply must drop its forwarder's broadcast receiver"
        );
    }
}
