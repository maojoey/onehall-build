//! Real owned child processes behind the existing ACP conversation loop.
//! No model, installed adapter, production runtime or user data is involved.

use super::*;
use agent_client_protocol::schema::v1::PromptResponse;
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, Notify};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonRpcRequest)]
#[request(method = "_session/async_task/stop", response = serde_json::Value)]
#[serde(transparent)]
struct StopTaskRequest(serde_json::Value);

#[derive(Default)]
struct Shells {
    children: BTreeMap<(String, String), Child>,
    stops: Vec<(String, String)>,
    refuse: bool,
    unpublished: usize,
}

impl Shells {
    async fn spawn(&mut self, session: &str, task: &str) -> u32 {
        let child = Command::new("sleep")
            .arg("120")
            .kill_on_drop(true)
            .spawn()
            .expect("owned test child");
        let pid = child.id().expect("child pid");
        self.children.insert((session.into(), task.into()), child);
        pid
    }

    fn running(&mut self, session: &str, task: &str) -> bool {
        self.children
            .get_mut(&(session.into(), task.into()))
            .expect("known test child")
            .try_wait()
            .expect("inspect child")
            .is_none()
    }
}

struct CancelLoop {
    state: Arc<RwLock<SessionState>>,
    commands: mpsc::Sender<ConnectionCommand>,
    wire: ConnectionTo<Client>,
    response: Arc<Mutex<Option<Responder<PromptResponse>>>>,
    prompted: Arc<Notify>,
    cancelled: Arc<Notify>,
    shells: Arc<Mutex<Shells>>,
    client: tokio::task::JoinHandle<()>,
    agent: tokio::task::JoinHandle<()>,
}

impl CancelLoop {
    async fn start() -> Self {
        Self::start_with_restored_shell(false).await
    }

    async fn start_with_restored_shell(restored: bool) -> Self {
        let (client_end, agent_end) = agent_client_protocol::Channel::duplex();
        let (wire_tx, wire_rx) = oneshot::channel();
        let response = Arc::new(Mutex::new(None));
        let prompted = Arc::new(Notify::new());
        let cancelled = Arc::new(Notify::new());
        let shells = Arc::new(Mutex::new(Shells::default()));
        let agent_response = Arc::clone(&response);
        let agent_prompted = Arc::clone(&prompted);
        let agent_cancelled = Arc::clone(&cancelled);
        let agent_shells = Arc::clone(&shells);
        let agent = tokio::spawn(async move {
            let _ = Agent
                .builder()
                .on_receive_request(
                    async |_req: NewSessionRequest,
                           responder: Responder<NewSessionResponse>,
                           _cx: ConnectionTo<Client>| {
                        responder.respond(NewSessionResponse::new(SessionId::new("s1")))
                    },
                    on_receive_request!(),
                )
                .on_receive_request(
                    async move |_req: PromptRequest,
                                responder: Responder<PromptResponse>,
                                _cx: ConnectionTo<Client>| {
                        *agent_response.lock().await = Some(responder);
                        agent_prompted.notify_one();
                        Ok(())
                    },
                    on_receive_request!(),
                )
                .on_receive_notification(
                    async move |_req: CancelNotification, _cx: ConnectionTo<Client>| {
                        agent_cancelled.notify_one();
                        Ok(())
                    },
                    on_receive_notification!(),
                )
                .on_receive_request(
                    async move |req: StopTaskRequest,
                                responder: Responder<serde_json::Value>,
                                cx: ConnectionTo<Client>| {
                        let sid = req.0["sessionId"].as_str().unwrap().to_string();
                        let task = req.0["asyncTaskId"].as_str().unwrap().to_string();
                        let mut shells = agent_shells.lock().await;
                        shells.stops.push((sid.clone(), task.clone()));
                        if shells.refuse {
                            return responder.respond(serde_json::json!({"stopped": false}));
                        }
                        if shells.unpublished > 0 {
                            shells.unpublished -= 1;
                            return responder.respond(serde_json::json!({"stopped": false}));
                        }
                        let child = shells.children.get_mut(&(sid.clone(), task.clone()));
                        let Some(child) = child else {
                            return responder.respond(serde_json::json!({"stopped": false}));
                        };
                        child.kill().await.expect("stop only the requested test child");
                        // kill() also waits/reaps: an acknowledgement here is
                        // backed by real process exit, not a state assignment.
                        send_update(&cx, &sid, serde_json::json!({
                            "sessionUpdate": "async_task_state_update",
                            "asyncTaskId": task,
                            "state": "stopped"
                        }))?;
                        responder.respond(serde_json::json!({"stopped": true}))
                    },
                    on_receive_request!(),
                )
                .connect_with(agent_end, async move |cx: ConnectionTo<Client>| {
                    let _ = wire_tx.send(cx);
                    std::future::pending::<Result<(), agent_client_protocol::Error>>().await
                })
                .await;
        });
        let state = Arc::new(RwLock::new(SessionState::new(
            "cancel-test".into(),
            AgentType::Codex,
            None,
            "test".into(),
            None,
        )));
        if restored {
            state.write().await.apply_event(&AcpEvent::AsyncTask {
                delta: serde_json::from_value(serde_json::json!({
                    "task_id": "restored", "tool_call_id": "restored",
                    "spawned": true, "task_type": "shell", "can_stop": true
                })).unwrap(),
            });
        }
        let loop_state = Arc::clone(&state);
        let (commands, mut command_rx) = mpsc::channel(16);
        let client = tokio::spawn(async move {
            let _ = Client
                .builder()
                .connect_with(client_end, async move |cx: ConnectionTo<Agent>| {
                    let raw = cx
                        .send_request_to(
                            Agent,
                            UntypedMessage::new("session/new", NewSessionRequest::new("/tmp"))?,
                        )
                        .block_task()
                        .await?;
                    let response: NewSessionResponse = serde_json::from_value(raw)
                        .map_err(agent_client_protocol::Error::into_internal_error)?;
                    let mut session = AgentSession::attach(&cx, response)?;
                    run_conversation_loop(
                        &mut session,
                        "cancel-test",
                        &EventEmitter::Noop,
                        &loop_state,
                        AgentType::Codex,
                        &PendingPermissions::default(),
                        &mut command_rx,
                        Arc::new(TerminalRuntime::with_base_env(BTreeMap::new())),
                        "/tmp",
                        false,
                        &[],
                        &background_watch::PromptLedger::shared(),
                        None,
                        &Arc::new(StderrTail::new()),
                    )
                    .await?;
                    Ok(())
                })
                .await;
        });
        Self {
            state,
            commands,
            wire: wire_rx.await.expect("fake adapter connected"),
            response,
            prompted,
            cancelled,
            shells,
            client,
            agent,
        }
    }

    async fn prompt(&self) {
        self.state.write().await.turn_in_flight = true;
        self.commands
            .send(ConnectionCommand::Prompt {
                blocks: vec![PromptInputBlock::Text { text: "test".into() }],
                user_message: None,
                internal_suffix: Vec::new(),
            })
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), self.prompted.notified())
            .await
            .expect("prompt received");
    }

    async fn tool(&self, id: &str) {
        send_update(&self.wire, "s1", serde_json::json!({
            "sessionUpdate": "tool_call", "toolCallId": id,
            "kind": "execute", "title": "sleep 120", "status": "in_progress"
        }))
        .unwrap();
        self.wait_for(|| async { self.state.read().await.active_tool_calls.contains_key(id) })
            .await;
    }

    fn publish(&self, session: &str, id: &str) {
        send_update(&self.wire, session, serde_json::json!({
            "sessionUpdate": "async_task_spawned", "asyncTaskId": id,
            "toolCallId": id, "name": "sleep 120", "taskType": "shell",
            "canStop": true, "showInTranscript": false
        }))
        .unwrap();
    }

    async fn finish(&self, reason: StopReason, turns: u64) {
        self.response.lock().await.take().unwrap()
            .respond(PromptResponse::new(reason)).unwrap();
        self.wait_for(|| async { self.state.read().await.turns_completed >= turns })
            .await;
    }

    async fn cancel(&self) {
        self.commands.send(ConnectionCommand::Cancel).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), self.cancelled.notified())
            .await
            .expect("ACP cancel reached the fake adapter");
    }

    async fn wait_for<F, Fut>(&self, mut condition: F)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !condition().await {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("observable condition within deadline");
    }

    async fn assert_stopped(&self, id: &str) {
        self.wait_for(|| async { !self.shells.lock().await.running("s1", id) }).await;
    }

    async fn old_background(&self) {
        self.shells.lock().await.spawn("s1", "old").await;
        self.prompt().await;
        self.tool("old").await;
        self.publish("s1", "old");
        self.finish(StopReason::EndTurn, 1).await;
    }
}

impl Drop for CancelLoop {
    fn drop(&mut self) {
        self.client.abort();
        self.agent.abort();
        // Child::kill_on_drop cleans ONLY this fixture's remaining children.
    }
}

fn send_update(
    cx: &ConnectionTo<Client>,
    sid: &str,
    update: serde_json::Value,
) -> Result<(), agent_client_protocol::Error> {
    cx.send_notification(UntypedMessage::new(
        "session/update",
        serde_json::json!({"sessionId": sid, "update": update}),
    )?)
}

#[tokio::test]
async fn cancelled_codex_turn_reaps_its_shell_and_preserves_old_and_other_sessions() {
    let h = CancelLoop::start().await;
    let other = CancelLoop::start().await;
    h.old_background().await;
    h.shells.lock().await.spawn("s1", "target").await;
    h.shells.lock().await.spawn("s2", "target").await;
    other.shells.lock().await.spawn("s1", "target").await;
    other.prompt().await;
    other.tool("target").await;
    other.publish("s1", "target");
    h.prompt().await;
    h.tool("target").await;
    h.publish("s1", "target");
    h.publish("s2", "target");
    h.cancel().await;
    h.finish(StopReason::Cancelled, 2).await;
    h.assert_stopped("target").await;
    assert!(h.shells.lock().await.running("s1", "old"));
    assert!(h.shells.lock().await.running("s2", "target"));
    assert!(other.shells.lock().await.running("s1", "target"));
    assert_eq!(h.shells.lock().await.stops, vec![("s1".into(), "target".into())]);
}

#[tokio::test]
async fn cancelled_codex_turn_reaps_a_command_which_starts_after_cancel() {
    let h = CancelLoop::start().await;
    h.old_background().await;
    h.prompt().await;
    h.cancel().await;
    assert!(h.state.read().await.turn_in_flight, "native acknowledgement still pending");
    // The actual child and even its tool announcement arrive across Cancel.
    h.shells.lock().await.spawn("s1", "late-start").await;
    h.tool("late-start").await;
    h.publish("s1", "late-start");
    h.finish(StopReason::Cancelled, 2).await;
    h.assert_stopped("late-start").await;
    assert!(h.shells.lock().await.running("s1", "old"));
}

#[tokio::test]
async fn late_codex_task_publication_does_not_cancel_the_next_turn() {
    for next_turn in [false, true] {
        let h = CancelLoop::start().await;
        h.old_background().await;
        h.prompt().await;
        h.tool("cancelled-tool").await;
        h.shells.lock().await.spawn("s1", "cancelled-tool").await;
        h.cancel().await;
        h.finish(StopReason::Cancelled, 2).await;
        assert!(h.shells.lock().await.running("s1", "cancelled-tool"));
        if next_turn {
            h.prompt().await;
            h.tool("next-tool").await;
            h.shells.lock().await.spawn("s1", "next-tool").await;
            h.publish("s1", "next-tool");
            // Replay an older call; it must not acquire this prompt's owner.
            h.tool("old").await;
            h.publish("s1", "old");
        }
        // Cover both the idle pump and the following prompt's update pump.
        h.publish("s1", "cancelled-tool");
        h.assert_stopped("cancelled-tool").await;
        assert!(h.shells.lock().await.running("s1", "old"));
        if next_turn {
            assert!(h.shells.lock().await.running("s1", "next-tool"));
            assert!(h.state.read().await.turn_in_flight);
            h.finish(StopReason::EndTurn, 3).await;
        }
    }
}

#[tokio::test]
async fn refused_codex_cleanup_reports_failure_instead_of_proving_exit_by_status() {
    let h = CancelLoop::start().await;
    h.shells.lock().await.refuse = true;
    h.prompt().await;
    h.tool("refused").await;
    h.shells.lock().await.spawn("s1", "refused").await;
    h.publish("s1", "refused");
    let mut events = h.state.read().await.event_stream().subscribe();
    h.cancel().await;
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if matches!(&events.recv().await.unwrap().payload,
                AcpEvent::Error { code: Some(code), .. }
                    if code == "codex_cancel_cleanup_unconfirmed") {
                break;
            }
        }
    }).await.expect("explicit cleanup failure");
    assert!(h.shells.lock().await.running("s1", "refused"));
    assert!(h.state.read().await.turn_in_flight);
    h.finish(StopReason::Cancelled, 1).await;
}

#[tokio::test]
async fn codex_cancel_does_not_adopt_unlinked_or_non_stoppable_work() {
    let h = CancelLoop::start().await;
    h.prompt().await;
    for id in ["target", "unlinked", "cannot-stop", "workflow"] {
        h.shells.lock().await.spawn("s1", id).await;
        if id != "unlinked" {
            h.tool(id).await;
        }
        send_update(&h.wire, "s1", serde_json::json!({
            "sessionUpdate": "async_task_spawned", "asyncTaskId": id,
            "toolCallId": id, "name": "sleep 120",
            "taskType": if id == "workflow" { "workflow" } else { "shell" },
            "canStop": id != "cannot-stop", "showInTranscript": false
        })).unwrap();
    }
    h.cancel().await;
    h.finish(StopReason::Cancelled, 1).await;
    h.assert_stopped("target").await;
    for id in ["unlinked", "cannot-stop", "workflow"] {
        assert!(h.shells.lock().await.running("s1", id), "{id} is not owned cancellable shell work");
    }
}

#[tokio::test]
async fn codex_cancel_retries_a_task_whose_publication_is_not_finished() {
    let h = CancelLoop::start().await;
    h.shells.lock().await.unpublished = 1;
    h.prompt().await;
    h.tool("publishing").await;
    h.shells.lock().await.spawn("s1", "publishing").await;
    h.publish("s1", "publishing");
    h.cancel().await;
    h.finish(StopReason::Cancelled, 1).await;
    h.assert_stopped("publishing").await;
    assert_eq!(h.shells.lock().await.stops.len(), 2);
}

#[tokio::test]
async fn codex_cancel_without_native_ack_reports_timeout_and_keeps_the_gate() {
    let h = CancelLoop::start().await;
    h.prompt().await;
    let mut events = h.state.read().await.event_stream().subscribe();
    h.cancel().await;
    tokio::time::timeout(std::time::Duration::from_secs(12), async {
        loop {
            if matches!(&events.recv().await.unwrap().payload,
                AcpEvent::Error { code: Some(code), .. }
                    if code == "codex_cancel_ack_timeout") {
                break;
            }
        }
    }).await.expect("explicit missing native acknowledgement");
    assert_eq!(h.state.read().await.turns_completed, 0);
    assert!(h.state.read().await.turn_in_flight);
    h.finish(StopReason::Cancelled, 1).await;
}

#[tokio::test]
async fn codex_cancel_preserves_a_restored_background_task() {
    let h = CancelLoop::start_with_restored_shell(true).await;
    h.shells.lock().await.spawn("s1", "restored").await;
    h.shells.lock().await.spawn("s1", "target").await;
    h.prompt().await;
    h.tool("restored").await;
    h.publish("s1", "restored");
    h.tool("target").await;
    h.publish("s1", "target");
    h.cancel().await;
    h.finish(StopReason::Cancelled, 1).await;
    h.assert_stopped("target").await;
    assert!(h.shells.lock().await.running("s1", "restored"));
}
