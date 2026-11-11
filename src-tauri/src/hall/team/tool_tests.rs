//! Model content tools against the real runtime fixture, exchange ledger and
//! blob store. Only the branch side is exercised: HQ staging produces the
//! manifest, and the bytes are received through the same core calls the v2
//! wire handler uses.
use super::exchange_wire::{bind_task, open, sql, uid, Hq, CONTENT, OUTPUT};
use super::{result, Fixture, Outcome};
use crate::hall::{
    exchange::{runtime::current_owner, DispatchOwner, Exchange, ExchangeManifest},
    tasks::runtime::authority::{Actor, Registry},
    team::{mcp, TeamService, ToolRequest},
};
use sea_orm::{ConnectionTrait, TransactionTrait};
use serde_json::{json, Value};
use std::sync::Arc;

struct Attached {
    f: Fixture,
    exchange: Exchange,
    service: TeamService,
    registry: Arc<Registry>,
    actor: Actor,
    manifest: ExchangeManifest,
    task: String,
    claim: crate::hall::tasks::queue::Claim,
    binding: crate::hall::tasks::runtime::Binding,
    _hq: Hq,
}
/// Branch-side receipt of one HQ-staged dispatch, bound to `task` the way
/// local acceptance does. Every call stages a new submission with the same
/// CONTENT, so blobs dedupe while access stays per object.
async fn attach(f: &Fixture, exchange: &Exchange, hq: &Hq, task: &str) -> ExchangeManifest {
    let (_, manifest) = hq.stage().await;
    let (issuer, dispatch, hq_branch) = (uid(), uid(), uid());
    let owner = DispatchOwner::authenticated(&issuer, &dispatch, &hq_branch).unwrap();
    let tx = f.db.begin().await.unwrap();
    exchange
        .accept_dispatch_in(&tx, &owner, &dispatch, &manifest)
        .await
        .unwrap();
    tx.execute(sql(
        "INSERT INTO hall_branch_inbox(issuer_id,dispatch_id,request_hash,member_ref,title,description,status,revision,created_at,updated_at) VALUES(?,?,?,?,?,?,'awaiting_confirmation',1,'t','t')",
        vec![issuer.clone().into(), dispatch.clone().into(), "h".into(), "worker".into(), "t".into(), "d".into()],
    ))
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let permit = exchange
        .prepare_dispatch_input(&owner, &manifest.inputs[0].object_id)
        .await
        .unwrap();
    let received = exchange
        .receive_dispatch_input(permit, CONTENT)
        .await
        .unwrap();
    let tx = f.db.begin().await.unwrap();
    exchange
        .commit_dispatch_input_in(&tx, received)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    bind_task(&f.db, &issuer, &dispatch, task).await;
    manifest
}
async fn attached() -> Attached {
    let f = Fixture::new().await;
    let exchange = open(&f.db, f._dir.path());
    f.co.attach_exchange(exchange.clone());
    let hq = Hq::new().await;
    let task = f.queued().await;
    let manifest = attach(&f, &exchange, &hq, &task).await;
    let (claim, binding) = f.co.dispatch_next().await.unwrap().unwrap();
    let registry = Arc::new(Registry::default());
    let actor = registry.fixture_actor(&f.db, &claim, &binding).await;
    let service =
        TeamService::new(f.host.clone(), registry.clone()).with_exchange(Some(exchange.clone()));
    Attached {
        f,
        exchange,
        service,
        registry,
        actor,
        manifest,
        task,
        claim,
        binding,
        _hq: hq,
    }
}
fn read(object: &str, offset: u64, max: Option<u64>) -> ToolRequest {
    ToolRequest::ReadInput {
        object_id: object.into(),
        offset,
        max_bytes: max,
    }
}

#[tokio::test]
async fn list_read_and_submit_use_only_the_hosts_current_run() {
    let a = attached().await;
    let listed = a
        .service
        .call(&a.actor, ToolRequest::ListInputs {})
        .await
        .unwrap();
    assert_eq!(listed, serde_json::to_value(&a.manifest.inputs).unwrap());
    let object = a.manifest.inputs[0].object_id.clone();
    let whole = a
        .service
        .call(&a.actor, read(&object, 0, None))
        .await
        .unwrap();
    assert_eq!(whole["text"].as_str().unwrap().as_bytes(), CONTENT);
    assert_eq!(
        (whole["eof"].clone(), whole["truncated"].clone()),
        (json!(true), json!(false))
    );
    // Chinese text in CONTENT is cut on a character boundary and resumes.
    let first = a
        .service
        .call(&a.actor, read(&object, 0, Some(40)))
        .await
        .unwrap();
    assert_eq!(first["truncated"], true);
    let next = first["nextOffset"].as_u64().unwrap();
    let rest = a
        .service
        .call(&a.actor, read(&object, next, None))
        .await
        .unwrap();
    assert_eq!(
        format!(
            "{}{}",
            first["text"].as_str().unwrap(),
            rest["text"].as_str().unwrap()
        )
        .as_bytes(),
        CONTENT
    );
    // Beyond EOF, or inside a character, is refused.
    assert!(a
        .service
        .call(&a.actor, read(&object, CONTENT.len() as u64 + 1, None))
        .await
        .is_err());
    let inside = CONTENT.iter().position(|b| *b >= 0xe4).unwrap() as u64 + 1;
    assert!(a
        .service
        .call(&a.actor, read(&object, inside, None))
        .await
        .is_err());
    // An object id that is not in this attempt's manifest is refused.
    assert!(a
        .service
        .call(&a.actor, read(&uid(), 0, None))
        .await
        .is_err());

    let slot = a.manifest.outputs[0].slot_id.clone();
    let (owner, exit) = current_owner(&a.f.db, &a.task).await.unwrap().unwrap();
    let paths = a.exchange.materialize(&owner).await.unwrap();
    std::fs::write(&paths.outputs[&slot], OUTPUT).unwrap();
    let submit = || ToolRequest::SubmitOutput {
        slot_id: slot.clone(),
    };
    let sealed = a.service.call(&a.actor, submit()).await.unwrap();
    assert_eq!(sealed["slotId"], json!(slot));
    assert_eq!(sealed["runId"], json!(a.binding.run_id()));
    assert_eq!(sealed["size"], json!(OUTPUT.len()));
    // Same content again returns the original artifact; later edits do not replace it.
    std::fs::write(&paths.outputs[&slot], b"changed after seal").unwrap();
    assert_eq!(a.service.call(&a.actor, submit()).await.unwrap(), sealed);
    assert_eq!(
        sealed["exitVerified"],
        json!(matches!(
            exit,
            crate::hall::exchange::ExitVerification::Verified
        ))
    );
    // Unknown slot ids and slot ids that are not UUIDs are refused.
    assert!(a
        .service
        .call(&a.actor, ToolRequest::SubmitOutput { slot_id: uid() })
        .await
        .is_err());
    assert!(a
        .service
        .call(
            &a.actor,
            ToolRequest::SubmitOutput {
                slot_id: "../x".into()
            }
        )
        .await
        .is_err());
}

#[tokio::test]
async fn stale_released_or_unattached_runs_and_missing_exchange_are_denied() {
    let a = attached().await;
    let object = a.manifest.inputs[0].object_id.clone();
    // Another task without any attachment has no inputs to list or read.
    let (m2, s2) = crate::hall::tasks_tests::fixture(&a.f.server, a.f._dir.path(), "other").await;
    let plain = a.f.queued_for(&m2, &s2).await;
    let (c2, b2) = a.f.co.dispatch_next().await.unwrap().unwrap();
    assert_eq!(c2.task_id(), plain);
    let other = a.registry.fixture_actor(&a.f.db, &c2, &b2).await;
    assert!(a
        .service
        .call(&other, ToolRequest::ListInputs {})
        .await
        .is_err());
    assert!(a
        .service
        .call(&other, ToolRequest::ListOutputs {})
        .await
        .is_err());
    assert!(a
        .service
        .call(&other, read(&object, 0, None))
        .await
        .is_err());
    assert!(a
        .service
        .call(
            &other,
            ToolRequest::SubmitOutput {
                slot_id: a.manifest.outputs[0].slot_id.clone()
            }
        )
        .await
        .is_err());
    // Without an exchange handle the tools deny rather than guess.
    let closed = TeamService::new(a.f.host.clone(), a.registry.clone());
    assert!(closed
        .call(&a.actor, ToolRequest::ListInputs {})
        .await
        .is_err());
    assert!(closed
        .call(&a.actor, ToolRequest::ListOutputs {})
        .await
        .is_err());
    // A revoked registry entry (released run) loses every content tool.
    a.registry.revoke(a.binding.run_id());
    assert!(a
        .service
        .call(&a.actor, ToolRequest::ListInputs {})
        .await
        .is_err());
    assert!(a
        .service
        .call(&a.actor, ToolRequest::ListOutputs {})
        .await
        .is_err());
    assert!(a
        .service
        .call(&a.actor, read(&object, 0, None))
        .await
        .is_err());
}

#[tokio::test]
async fn submission_after_the_result_is_accepted_and_exit_confirmed_is_refused() {
    let a = attached().await;
    let slot = a.manifest.outputs[0].slot_id.clone();
    let (owner, _) = current_owner(&a.f.db, &a.task).await.unwrap().unwrap();
    let paths = a.exchange.materialize(&owner).await.unwrap();
    std::fs::write(&paths.outputs[&slot], OUTPUT).unwrap();
    a.f.co
        .accept_event(&a.claim, &a.binding, 1, result(Outcome::Succeeded))
        .await
        .unwrap();
    a.f.port
        .exited
        .store(true, std::sync::atomic::Ordering::SeqCst);
    a.f.co.confirm_exit(&a.claim, &a.binding).await.unwrap();
    assert!(a
        .service
        .call(&a.actor, ToolRequest::SubmitOutput { slot_id: slot })
        .await
        .is_err());
}

#[tokio::test]
async fn a_retried_task_gives_content_tools_only_to_the_new_attempt() {
    let a = attached().await;
    let object = a.manifest.inputs[0].object_id.clone();
    let slot = a.manifest.outputs[0].slot_id.clone();
    a.f.co
        .accept_event(&a.claim, &a.binding, 1, result(Outcome::Failed))
        .await
        .unwrap();
    a.f.port
        .exited
        .store(true, std::sync::atomic::Ordering::SeqCst);
    a.f.co.confirm_exit(&a.claim, &a.binding).await.unwrap();
    let revision = a.f.detail(&a.task).await["revision"].as_i64().unwrap();
    a.f.host.enqueue(&a.task, revision, &uid()).await.unwrap();
    let (c2, b2) = a.f.co.dispatch_next().await.unwrap().unwrap();
    assert_eq!(c2.task_id(), a.task);
    assert_ne!(b2.run_id(), a.binding.run_id());
    // This registry never revoked the old run: the host's current-attempt and
    // run checks alone must refuse it.
    assert!(a.registry.own_task(&a.actor).is_ok());
    assert!(a
        .service
        .call(&a.actor, ToolRequest::OwnTask {})
        .await
        .is_err());
    assert!(a
        .service
        .call(&a.actor, ToolRequest::ListInputs {})
        .await
        .is_err());
    assert!(a
        .service
        .call(&a.actor, ToolRequest::ListOutputs {})
        .await
        .is_err());
    assert!(a
        .service
        .call(&a.actor, read(&object, 0, None))
        .await
        .is_err());
    assert!(a
        .service
        .call(
            &a.actor,
            ToolRequest::SubmitOutput {
                slot_id: slot.clone()
            }
        )
        .await
        .is_err());
    let fresh = a.registry.fixture_actor(&a.f.db, &c2, &b2).await;
    let listed = a
        .service
        .call(&fresh, ToolRequest::ListInputs {})
        .await
        .unwrap();
    assert_eq!(listed, serde_json::to_value(&a.manifest.inputs).unwrap());
    let (owner, _) = current_owner(&a.f.db, &a.task).await.unwrap().unwrap();
    let paths = a.exchange.materialize(&owner).await.unwrap();
    std::fs::write(&paths.outputs[&slot], OUTPUT).unwrap();
    let sealed = a
        .service
        .call(&fresh, ToolRequest::SubmitOutput { slot_id: slot })
        .await
        .unwrap();
    assert_eq!(sealed["runId"], json!(b2.run_id()));
    assert_eq!(sealed["attemptId"], json!(c2.attempt_id()));
}

#[tokio::test]
async fn a_live_actor_whose_run_is_not_the_attempts_recorded_run_is_refused() {
    let a = attached().await;
    let object = a.manifest.inputs[0].object_id.clone();
    let slot = a.manifest.outputs[0].slot_id.clone();
    // The host now records another run for this same, still current attempt;
    // the registry still holds the old run bound to its connection.
    a.f.db
        .execute(sql(
            "UPDATE hall_task_runtime SET run_id=? WHERE attempt_id=?",
            vec![uid().into(), a.claim.attempt_id().into()],
        ))
        .await
        .unwrap();
    assert!(a.registry.own_task(&a.actor).is_ok());
    assert!(a
        .service
        .call(&a.actor, ToolRequest::OwnTask {})
        .await
        .is_err());
    assert!(a
        .service
        .call(&a.actor, ToolRequest::ListInputs {})
        .await
        .is_err());
    assert!(a
        .service
        .call(&a.actor, ToolRequest::ListOutputs {})
        .await
        .is_err());
    assert!(a
        .service
        .call(&a.actor, read(&object, 0, None))
        .await
        .is_err());
    assert!(a
        .service
        .call(&a.actor, ToolRequest::SubmitOutput { slot_id: slot })
        .await
        .is_err());
}

#[tokio::test]
async fn members_and_the_pm_cannot_use_another_tasks_objects_or_slots() {
    let a = attached().await;
    // A second member, registered as PM, with its own attachment of identical
    // bytes. The PM role grants no reach into the first member's objects or
    // slots: it is still resolved to its own run only.
    let (m2, s2) = crate::hall::tasks_tests::fixture(&a.f.server, a.f._dir.path(), "other").await;
    let second = a.f.queued_for(&m2, &s2).await;
    let other_manifest = attach(&a.f, &a.exchange, &a._hq, &second).await;
    let (c2, b2) = a.f.co.dispatch_next().await.unwrap().unwrap();
    assert_eq!(c2.task_id(), second);
    a.registry
        .configure_fixture_pm(m2["memberId"].as_str().unwrap());
    let other = a.registry.fixture_actor(&a.f.db, &c2, &b2).await;
    assert!(a.registry.authorize_pm(&other).is_ok());
    assert!(a.registry.authorize_pm(&a.actor).is_err());
    let (mine, theirs) = (&a.manifest, &other_manifest);
    assert_eq!(
        mine.inputs[0].sha256, theirs.inputs[0].sha256,
        "same content, distinct objects"
    );
    assert_ne!(mine.inputs[0].object_id, theirs.inputs[0].object_id);
    // Each member sees only its own inputs and slots.
    let listed = a
        .service
        .call(&other, ToolRequest::ListInputs {})
        .await
        .unwrap();
    assert_eq!(listed, serde_json::to_value(&theirs.inputs).unwrap());
    assert!(a
        .service
        .call(&other, read(&mine.inputs[0].object_id, 0, None))
        .await
        .is_err());
    assert!(a
        .service
        .call(&a.actor, read(&theirs.inputs[0].object_id, 0, None))
        .await
        .is_err());
    assert!(a
        .service
        .call(
            &other,
            ToolRequest::SubmitOutput {
                slot_id: mine.outputs[0].slot_id.clone()
            }
        )
        .await
        .is_err());
    assert!(a
        .service
        .call(
            &a.actor,
            ToolRequest::SubmitOutput {
                slot_id: theirs.outputs[0].slot_id.clone()
            }
        )
        .await
        .is_err());
    assert!(a
        .service
        .call(&other, read(&theirs.inputs[0].object_id, 0, None))
        .await
        .is_ok());
}

#[test]
fn tool_arguments_never_carry_identity_paths_or_addresses() {
    for (tool, args) in [
        ("hall_list_inputs", json!({})),
        ("hall_list_outputs", json!({})),
        (
            "hall_read_input",
            json!({"objectId": uid(), "offset": 0, "maxBytes": 100}),
        ),
        ("hall_read_input", json!({"objectId": uid()})),
        ("hall_submit_output", json!({"slotId": uid()})),
    ] {
        assert!(mcp::input(tool, args).is_ok(), "{tool}");
    }
    for forged in [
        "taskId",
        "attemptId",
        "runId",
        "path",
        "outputPath",
        "branchAddress",
        "action",
        "dispatchId",
    ] {
        for (tool, base) in [
            ("hall_list_inputs", json!({})),
            ("hall_list_outputs", json!({})),
            ("hall_read_input", json!({"objectId": uid()})),
            ("hall_submit_output", json!({"slotId": uid()})),
        ] {
            let mut args = base;
            args.as_object_mut()
                .unwrap()
                .insert(forged.into(), json!("x"));
            assert!(mcp::input(tool, args).is_err(), "{tool} accepted {forged}");
        }
    }
    // Snake-case and wrong-typed arguments do not slip through either.
    assert!(mcp::input("hall_read_input", json!({"object_id": uid()})).is_err());
    assert!(mcp::input("hall_read_input", json!({"objectId": uid(), "offset": -1})).is_err());
    assert!(mcp::input("hall_submit_output", json!({"slotId": 7})).is_err());
}

#[test]
fn only_these_tools_are_member_tools_and_only_they_are_auto_allowed() {
    let names = [
        "hall_list_inputs",
        "hall_list_outputs",
        "hall_read_input",
        "hall_submit_output",
    ];
    for name in names {
        assert!(mcp::member_tool(name) && !mcp::pm_tool(name));
        // Codex card shape: server + tool + arguments.
        assert!(mcp::auto_allowed(
            &json!({"server":"codeg-mcp","tool":name,"arguments":{}})
        ));
        assert!(!mcp::auto_allowed(
            &json!({"server":"other","tool":name,"arguments":{}})
        ));
        // Claude Code card shape: the full tool name.
        assert!(mcp::auto_allowed_tool_name(&format!(
            "mcp__codeg-mcp__{name}"
        )));
        assert!(!mcp::auto_allowed_tool_name(&format!("mcp__other__{name}")));
        assert!(!mcp::auto_allowed_tool_name(&format!(
            "mcp__codeg-mcp__{name}2"
        )));
    }
    // Ordinary tools of the same server are not swept in by the new ones.
    for other in [
        "delegate_to_agent",
        "hall_read_inputs",
        "hall_submit",
        "computer_click",
    ] {
        assert!(!mcp::member_tool(other) && !mcp::pm_tool(other));
        assert!(!mcp::auto_allowed(
            &json!({"server":"codeg-mcp","tool":other,"arguments":{}})
        ));
        assert!(!mcp::auto_allowed_tool_name(&format!(
            "mcp__codeg-mcp__{other}"
        )));
    }
    let schemas = mcp::schemas();
    for name in names {
        let schema = schemas.iter().find(|s| s["name"] == name).expect("schema");
        assert_eq!(schema["inputSchema"]["additionalProperties"], false);
        let props = schema["inputSchema"]["properties"].as_object().unwrap();
        assert!(props
            .keys()
            .all(|k| ["objectId", "offset", "maxBytes", "slotId"].contains(&k.as_str())));
    }
    let _: Value = json!(null);
}

#[tokio::test]
async fn the_model_can_learn_its_output_slots_and_seal_what_it_wrote_there() {
    let a = attached().await;
    let (owner, _) = current_owner(&a.f.db, &a.task).await.unwrap().unwrap();
    // The host prepares the attempt directories before any model starts.
    a.exchange.materialize(&owner).await.unwrap();
    let listed = a
        .service
        .call(&a.actor, ToolRequest::ListOutputs {})
        .await
        .unwrap();
    let rows = listed.as_array().unwrap();
    assert_eq!(rows.len(), a.manifest.outputs.len());
    let slot = a.manifest.outputs[0].slot_id.clone();
    let row = rows.iter().find(|r| r["slotId"] == json!(slot)).unwrap();
    assert_eq!(
        row["displayName"],
        json!(a.manifest.outputs[0].display_name)
    );
    let path = std::path::PathBuf::from(row["path"].as_str().unwrap());
    assert!(path.is_absolute() && path.parent().unwrap().is_dir());
    assert_eq!(
        path.file_name().unwrap().to_str().unwrap(),
        a.manifest.outputs[0].display_name
    );
    // Writing exactly where the host said, then asking for the slot, seals it.
    std::fs::write(&path, OUTPUT).unwrap();
    let sealed = a
        .service
        .call(
            &a.actor,
            ToolRequest::SubmitOutput {
                slot_id: slot.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(sealed["size"], json!(OUTPUT.len()));
    // Only the tool's own fields are accepted: identity or paths never come from the model.
    assert!(mcp::input("hall_list_outputs", json!({"path": "/etc"})).is_err());
    assert!(mcp::input("hall_list_outputs", json!({})).is_ok());
    assert!(mcp::member_tool("hall_list_outputs") && !mcp::pm_tool("hall_list_outputs"));
}

#[tokio::test]
async fn revoked_exchange_attempt_no_longer_discloses_writable_output_targets() {
    let a = attached().await;
    let (owner, _) = current_owner(&a.f.db, &a.task).await.unwrap().unwrap();
    a.exchange.materialize(&owner).await.unwrap();
    assert!(a
        .service
        .call(&a.actor, ToolRequest::ListOutputs {})
        .await
        .is_ok());
    a.exchange.revoke_attempt(&owner).await.unwrap();
    assert!(a
        .service
        .call(&a.actor, ToolRequest::ListOutputs {})
        .await
        .is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn moved_output_ancestors_do_not_yield_a_stale_host_path() {
    let a = attached().await;
    let (owner, _) = current_owner(&a.f.db, &a.task).await.unwrap().unwrap();
    let paths = a.exchange.materialize(&owner).await.unwrap();
    let output = paths.outputs.values().next().unwrap();
    let root = output.ancestors().nth(5).unwrap();
    std::fs::rename(root, root.with_extension("moved")).unwrap();
    assert!(a
        .service
        .call(&a.actor, ToolRequest::ListOutputs {})
        .await
        .is_err());
}
