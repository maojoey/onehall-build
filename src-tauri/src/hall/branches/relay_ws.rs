use super::{relay::Runtime, relay_error::RelayError, service};
use crate::models::ToHeaderMap;
use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket},
        Extension, Path, WebSocketUpgrade,
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::{Sink, SinkExt, StreamExt};
use std::{sync::Arc, time::Duration};
use tokio::sync::OwnedSemaphorePermit;
use tokio_tungstenite::{
    tungstenite::{client::IntoClientRequest, Message as UpstreamMessage},
    MaybeTlsStream, WebSocketStream,
};

// Match the existing direct desktop tungstenite transport. Snapshots include
// running tool data and generated images beyond the finished-tool text budget.
const EVENT_MESSAGE_LIMIT: usize = 64 * 1024 * 1024;
const EVENT_FRAME_LIMIT: usize = 16 * 1024 * 1024;

type Upstream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
pub(super) async fn events(
    Extension(runtime): Extension<Arc<Runtime>>,
    Path(branch): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response, RelayError> {
    let permit = runtime.ws.clone().try_acquire_owned().map_err(|_| {
        tracing::warn!(
            side = "client",
            reason = "busy",
            "branch event relay rejected"
        );
        RelayError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "branch_busy",
            "Branch event relay is busy; retry shortly",
        )
    })?;
    let target = service::target(&runtime.db, &branch).await?;
    let mut url = reqwest::Url::parse(&format!("{}/ws/events", target.connection.base_url))
        .map_err(|_| RelayError::invalid_response())?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme)
        .map_err(|_| RelayError::invalid_response())?;
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|_| RelayError::invalid_response())?;
    request
        .headers_mut()
        .extend(target.connection.headers.to_header_map());
    let mut bearer =
        http::HeaderValue::from_str(&format!("Bearer {}", target.connection.token.trim()))
            .map_err(|_| RelayError::invalid_response())?;
    bearer.set_sensitive(true);
    request
        .headers_mut()
        .insert(http::header::AUTHORIZATION, bearer);
    request.headers_mut().insert(
        http::header::SEC_WEBSOCKET_PROTOCOL,
        http::HeaderValue::from_static(crate::web::auth::WS_EVENT_PROTOCOL),
    );
    let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(EVENT_MESSAGE_LIMIT))
        .max_frame_size(Some(EVENT_FRAME_LIMIT));
    // Establish/authenticate upstream before accepting the operator's socket.
    let (upstream, response) = tokio::time::timeout(
        Duration::from_secs(8),
        tokio_tungstenite::connect_async_with_config(request, Some(config), false),
    )
    .await
    .map_err(|_| {
        RelayError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "branch_timeout",
            "Branch event connection timed out",
        )
    })?
    .map_err(|e| match e {
        tokio_tungstenite::tungstenite::Error::Http(r)
            if r.status() == StatusCode::UNAUTHORIZED =>
        {
            RelayError::new(
                StatusCode::BAD_GATEWAY,
                "branch_auth_failed",
                "Branch credentials were rejected",
            )
        }
        _ => RelayError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "branch_unreachable",
            "Branch events could not be reached",
        ),
    })?;
    if response
        .headers()
        .get(http::header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        != Some(crate::web::auth::WS_EVENT_PROTOCOL)
    {
        return Err(RelayError::invalid_response());
    }
    Ok(ws
        .protocols([crate::web::auth::WS_EVENT_PROTOCOL])
        .max_message_size(EVENT_MESSAGE_LIMIT)
        .max_frame_size(EVENT_FRAME_LIMIT)
        .on_upgrade(move |client| exchange(client, upstream, permit))
        .into_response())
}
// Each leg is probed independently. A healthy leg must never keep a silent
// opposite peer alive, and relay control responses never cross the relay.
#[derive(Clone, Copy)]
struct HeartbeatPolicy {
    interval: Duration,
    silence: Duration,
}

impl Default for HeartbeatPolicy {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(15),
            silence: Duration::from_secs(45),
        }
    }
}

const SEND_TIMEOUT: Duration = Duration::from_secs(8);
const CLIENT_HEARTBEAT: &[u8] = b"codeg-relay-client";
const UPSTREAM_HEARTBEAT: &[u8] = b"codeg-relay-upstream";

async fn relay_send<S, M>(sink: &mut S, message: M, side: &'static str, bytes: usize) -> bool
where
    S: Sink<M> + Unpin,
{
    let started = tokio::time::Instant::now();
    let reason = match tokio::time::timeout(SEND_TIMEOUT, sink.send(message)).await {
        Ok(Ok(())) => return true,
        Ok(Err(_)) => "send_failed",
        Err(_) => "send_timeout",
    };
    tracing::warn!(
        side,
        reason,
        bytes,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "branch event relay stopped"
    );
    false
}

async fn exchange(client: WebSocket, upstream: Upstream, permit: OwnedSemaphorePermit) {
    exchange_with_policy(client, upstream, permit, HeartbeatPolicy::default()).await;
}

// Each leg owns its writer. Bounded one-frame handoffs keep a slow writer
// from preventing the opposite reader from draining a simultaneous send.
struct RelayWrite<M> {
    message: M,
    bytes: usize,
    closed: Option<tokio::sync::oneshot::Sender<()>>,
}

async fn queue_relay_write<M>(
    sender: &tokio::sync::mpsc::Sender<RelayWrite<M>>,
    message: M,
    bytes: usize,
    close: bool,
    side: &'static str,
) -> bool {
    let (closed, acknowledgment) = if close {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };
    if !matches!(
        tokio::time::timeout(
            SEND_TIMEOUT,
            sender.send(RelayWrite {
                message,
                bytes,
                closed
            })
        )
        .await,
        Ok(Ok(()))
    ) {
        tracing::warn!(
            side,
            reason = "handoff_failed",
            bytes,
            "branch event relay stopped"
        );
        return false;
    }
    if let Some(acknowledgment) = acknowledgment {
        // Keep the reader alive until its writer actually forwards Close.
        let _ = tokio::time::timeout(SEND_TIMEOUT, acknowledgment).await;
        return false;
    }
    true
}

async fn write_relay_leg<S, M>(
    mut sink: S,
    mut writes: tokio::sync::mpsc::Receiver<RelayWrite<M>>,
    last_seen: tokio::sync::watch::Receiver<tokio::time::Instant>,
    policy: HeartbeatPolicy,
    side: &'static str,
    ping: M,
) where
    S: Sink<M> + Unpin,
    M: Clone,
{
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + policy.interval,
        policy.interval,
    );
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            write = writes.recv() => {
                let Some(write) = write else { break };
                if !relay_send(&mut sink, write.message, side, write.bytes).await { break; }
                if let Some(closed) = write.closed {
                    let _ = closed.send(());
                    break;
                }
            }
            _ = heartbeat.tick() => {
                let elapsed = last_seen.borrow().elapsed();
                if elapsed >= policy.silence {
                    tracing::warn!(side, reason = "heartbeat_timeout", elapsed_ms = elapsed.as_millis() as u64, "branch event relay stopped");
                    break;
                }
                if !relay_send(&mut sink, ping.clone(), side, CLIENT_HEARTBEAT.len()).await { break; }
            }
        }
    }
}

async fn exchange_with_policy(
    client: WebSocket,
    upstream: Upstream,
    _permit: OwnedSemaphorePermit,
    policy: HeartbeatPolicy,
) {
    let (client_tx, mut client_rx) = client.split();
    let (upstream_tx, mut upstream_rx) = upstream.split();
    let (to_client, client_writes) = tokio::sync::mpsc::channel(1);
    let (to_upstream, upstream_writes) = tokio::sync::mpsc::channel(1);
    let (client_seen, client_last_seen) = tokio::sync::watch::channel(tokio::time::Instant::now());
    let (upstream_seen, upstream_last_seen) =
        tokio::sync::watch::channel(tokio::time::Instant::now());

    let client_reader = async {
        while let Some(Ok(message)) = client_rx.next().await {
            client_seen.send_replace(tokio::time::Instant::now());
            let (message, bytes, close) = match message {
                Message::Text(text) => {
                    let n = text.len();
                    (UpstreamMessage::Text(text.to_string().into()), n, false)
                }
                Message::Binary(bytes) => {
                    let n = bytes.len();
                    (UpstreamMessage::Binary(bytes.to_vec().into()), n, false)
                }
                Message::Close(frame) => {
                    let n = frame.as_ref().map_or(0, |f| f.reason.len());
                    (
                        UpstreamMessage::Close(frame.map(|f| {
                            tokio_tungstenite::tungstenite::protocol::CloseFrame {
                                code: f.code.into(),
                                reason: f.reason.to_string().into(),
                            }
                        })),
                        n,
                        true,
                    )
                }
                Message::Ping(bytes) => {
                    let n = bytes.len();
                    if !queue_relay_write(&to_client, Message::Pong(bytes), n, false, "client")
                        .await
                    {
                        break;
                    }
                    continue;
                }
                Message::Pong(_) => continue,
            };
            if !queue_relay_write(&to_upstream, message, bytes, close, "upstream").await {
                break;
            }
        }
        tracing::debug!(
            side = "client",
            reason = "reader_finished",
            "branch event relay stopped"
        );
    };
    let upstream_reader = async {
        while let Some(Ok(message)) = upstream_rx.next().await {
            upstream_seen.send_replace(tokio::time::Instant::now());
            let (message, bytes, close) = match message {
                UpstreamMessage::Text(text) => {
                    let n = text.len();
                    (Message::Text(text.to_string().into()), n, false)
                }
                UpstreamMessage::Binary(bytes) => {
                    let n = bytes.len();
                    (Message::Binary(bytes.to_vec().into()), n, false)
                }
                UpstreamMessage::Close(frame) => {
                    let n = frame.as_ref().map_or(0, |f| f.reason.len());
                    (
                        Message::Close(frame.map(|f| CloseFrame {
                            code: f.code.into(),
                            reason: f.reason.to_string().into(),
                        })),
                        n,
                        true,
                    )
                }
                UpstreamMessage::Ping(bytes) => {
                    let n = bytes.len();
                    if !queue_relay_write(
                        &to_upstream,
                        UpstreamMessage::Pong(bytes),
                        n,
                        false,
                        "upstream",
                    )
                    .await
                    {
                        break;
                    }
                    continue;
                }
                UpstreamMessage::Pong(_) | UpstreamMessage::Frame(_) => continue,
            };
            if !queue_relay_write(&to_client, message, bytes, close, "client").await {
                break;
            }
        }
        tracing::debug!(
            side = "upstream",
            reason = "reader_finished",
            "branch event relay stopped"
        );
    };
    let client_writer = write_relay_leg(
        client_tx,
        client_writes,
        client_last_seen,
        policy,
        "client",
        Message::Ping(CLIENT_HEARTBEAT.to_vec().into()),
    );
    let upstream_writer = write_relay_leg(
        upstream_tx,
        upstream_writes,
        upstream_last_seen,
        policy,
        "upstream",
        UpstreamMessage::Ping(UPSTREAM_HEARTBEAT.to_vec().into()),
    );
    // Futures are connection-owned: cancellation drops both split sockets,
    // bounded queues and the permit, with no detached tasks left behind.
    tokio::select! {
        _ = client_reader => {},
        _ = upstream_reader => {},
        _ = client_writer => {},
        _ = upstream_writer => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};
    use tokio::{
        sync::{oneshot, Semaphore},
        task::JoinHandle,
    };

    struct SocketPair {
        server: WebSocket,
        peer: Upstream,
        listener: JoinHandle<()>,
    }

    async fn socket_pair() -> SocketPair {
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(tokio::sync::Mutex::new(Some(tx)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/",
            get(move |ws: WebSocketUpgrade| {
                let tx = tx.clone();
                async move {
                    ws.on_upgrade(move |socket| async move {
                        let _ = tx.lock().await.take().unwrap().send(socket);
                    })
                }
            }),
        );
        let listener = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (peer, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
            .await
            .unwrap();
        // Keep the duplex regression independent of OS TCP autotuning: both
        // send-then-read peers must fill buffers before their complete frame
        // can be forwarded. Only temporary test sockets are affected.
        #[cfg(unix)]
        if let MaybeTlsStream::Plain(tcp) = peer.get_ref() {
            use std::os::fd::AsRawFd;
            let size: libc::c_int = 64 * 1024;
            for option in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
                assert_eq!(
                    unsafe {
                        libc::setsockopt(
                            tcp.as_raw_fd(),
                            libc::SOL_SOCKET,
                            option,
                            &size as *const _ as *const _,
                            std::mem::size_of_val(&size) as libc::socklen_t,
                        )
                    },
                    0
                );
            }
        }
        SocketPair {
            server: rx.await.unwrap(),
            peer,
            listener,
        }
    }

    fn short_policy() -> HeartbeatPolicy {
        HeartbeatPolicy {
            interval: Duration::from_millis(20),
            silence: Duration::from_millis(100),
        }
    }

    async fn run_pair() -> (
        Upstream,
        WebSocket,
        JoinHandle<()>,
        Arc<Semaphore>,
        Vec<JoinHandle<()>>,
    ) {
        run_pair_with_policy(short_policy()).await
    }

    async fn run_pair_with_policy(
        policy: HeartbeatPolicy,
    ) -> (
        Upstream,
        WebSocket,
        JoinHandle<()>,
        Arc<Semaphore>,
        Vec<JoinHandle<()>>,
    ) {
        let client = socket_pair().await;
        let upstream = socket_pair().await;
        let permits = Arc::new(Semaphore::new(1));
        let permit = permits.clone().acquire_owned().await.unwrap();
        let exchange = tokio::spawn(exchange_with_policy(
            client.server,
            upstream.peer,
            permit,
            policy,
        ));
        (
            client.peer,
            upstream.server,
            exchange,
            permits,
            vec![client.listener, upstream.listener],
        )
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn simultaneous_large_messages_keep_both_directions_moving() {
        let (mut client, mut upstream, exchange, permits, listeners) =
            run_pair_with_policy(HeartbeatPolicy::default()).await;
        let received = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(
                async {
                    client
                        .send(UpstreamMessage::Binary(vec![7; 6 * 1024 * 1024].into()))
                        .await
                        .unwrap();
                    client.next().await.unwrap().unwrap()
                },
                async {
                    upstream
                        .send(Message::Binary(vec![9; 6 * 1024 * 1024].into()))
                        .await
                        .unwrap();
                    upstream.recv().await.unwrap().unwrap()
                }
            )
        })
        .await
        .expect("simultaneous send-then-read peers must not wait for relay send timeout");
        assert!(
            matches!(received.0, UpstreamMessage::Binary(bytes) if bytes.len() == 6 * 1024 * 1024 && bytes[0] == 9)
        );
        assert!(
            matches!(received.1, Message::Binary(bytes) if bytes.len() == 6 * 1024 * 1024 && bytes[0] == 7)
        );
        drop(client);
        drop(upstream);
        tokio::time::timeout(Duration::from_secs(1), exchange)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(permits.available_permits(), 1);
        for listener in listeners {
            listener.abort();
        }
    }

    async fn answer_client(mut peer: Upstream) {
        while let Some(Ok(frame)) = peer.next().await {
            match frame {
                UpstreamMessage::Ping(bytes) => {
                    assert_eq!(bytes.as_ref(), b"codeg-relay-client");
                    if peer.send(UpstreamMessage::Pong(bytes)).await.is_err() {
                        break;
                    }
                }
                UpstreamMessage::Close(_) => break,
                other => panic!("unexpected client-side frame: {other:?}"),
            }
        }
    }

    async fn answer_upstream(mut peer: WebSocket) {
        while let Some(Ok(frame)) = peer.recv().await {
            match frame {
                Message::Ping(bytes) => {
                    assert_eq!(bytes.as_ref(), b"codeg-relay-upstream");
                    if peer.send(Message::Pong(bytes)).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                other => panic!("unexpected upstream-side frame: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn silent_client_releases_relay_permit() {
        let (_silent_client, upstream, exchange, permits, listeners) = run_pair().await;
        let responder = tokio::spawn(answer_upstream(upstream));
        tokio::time::timeout(Duration::from_secs(2), exchange)
            .await
            .expect("silent client must expire")
            .unwrap();
        assert_eq!(permits.available_permits(), 1);
        responder.await.unwrap();
        for listener in listeners {
            listener.abort();
        }
    }

    #[tokio::test]
    async fn silent_upstream_releases_relay_permit() {
        let (client, _silent_upstream, exchange, permits, listeners) = run_pair().await;
        let responder = tokio::spawn(answer_client(client));
        tokio::time::timeout(Duration::from_secs(2), exchange)
            .await
            .expect("silent upstream must expire")
            .unwrap();
        assert_eq!(permits.available_permits(), 1);
        responder.await.unwrap();
        for listener in listeners {
            listener.abort();
        }
    }

    #[tokio::test]
    async fn responsive_idle_peers_stay_connected_without_cross_leg_pongs() {
        let (mut client, mut upstream, exchange, permits, listeners) = run_pair().await;
        // Unsolicited control responses are local to their leg as well.
        client
            .send(UpstreamMessage::Pong(b"client-local".to_vec().into()))
            .await
            .unwrap();
        upstream
            .send(Message::Pong(b"upstream-local".to_vec().into()))
            .await
            .unwrap();
        let client = tokio::spawn(answer_client(client));
        let upstream = tokio::spawn(answer_upstream(upstream));
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert!(
            !exchange.is_finished(),
            "responsive idle sockets must survive several silence windows"
        );
        assert_eq!(permits.available_permits(), 0);
        exchange.abort();
        let _ = exchange.await;
        assert_eq!(permits.available_permits(), 1);
        client.abort();
        upstream.abort();
        for listener in listeners {
            listener.abort();
        }
    }

    #[tokio::test]
    async fn relay_preserves_text_binary_and_close_frames() {
        let (mut client, mut upstream, exchange, permits, listeners) = run_pair().await;
        client
            .send(UpstreamMessage::Text("attach".into()))
            .await
            .unwrap();
        assert_eq!(
            upstream.recv().await.unwrap().unwrap(),
            Message::Text("attach".into())
        );
        upstream
            .send(Message::Binary(vec![0, 255, 128].into()))
            .await
            .unwrap();
        assert_eq!(
            client.next().await.unwrap().unwrap(),
            UpstreamMessage::Binary(vec![0, 255, 128].into())
        );
        client
            .send(UpstreamMessage::Close(Some(
                tokio_tungstenite::tungstenite::protocol::CloseFrame {
                    code: 1000.into(),
                    reason: "done".into(),
                },
            )))
            .await
            .unwrap();
        assert_eq!(
            upstream.recv().await.unwrap().unwrap(),
            Message::Close(Some(CloseFrame {
                code: 1000,
                reason: "done".into()
            }))
        );
        tokio::time::timeout(Duration::from_secs(2), exchange)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(permits.available_permits(), 1);
        for listener in listeners {
            listener.abort();
        }
    }
    #[tokio::test(start_paused = true)]
    async fn stalled_relay_writer_exits_at_deadline() {
        let mut sink = Box::pin(futures::sink::unfold((), |(), _: Message| async {
            std::future::pending::<Result<(), ()>>().await
        }));
        let started = tokio::time::Instant::now();
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(9),
                relay_send(&mut sink, Message::Close(None), "client", 0)
            )
            .await,
            Ok(false)
        );
        assert_eq!(started.elapsed(), Duration::from_secs(8));
    }
}
