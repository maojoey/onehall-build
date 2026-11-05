use super::*;
use sha2::{Digest, Sha256};
const CONTENT: &[u8] = "{\"numbers\":[3,5,8,13],\"note\":\"一堂文件中转验收\"}".as_bytes();
fn hash(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
fn uid() -> String {
    uuid::Uuid::new_v4().to_string()
}
struct Fixture {
    store: Exchange,
    db: sea_orm::DatabaseConnection,
    _dir: tempfile::TempDir,
    admin: Administrator,
    submission: String,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::test_helpers::fresh_disk_db(dir.path())
            .await
            .conn;
        #[cfg(unix)]
        let root = dir.path().canonicalize().unwrap().join("exchange");
        #[cfg(windows)]
        let root = dir.path().join("exchange");
        let store = Exchange::open(db.clone(), &root).unwrap();
        Self {
            _dir: dir,
            db,
            store,
            admin: Administrator::authenticated("admin-a").unwrap(),
            submission: uid(),
        }
    }
    async fn input(&self, name: &str, bytes: &[u8]) -> ObjectRef {
        let ticket = self
            .store
            .prepare_input(
                &self.admin,
                &self.submission,
                name,
                bytes.len() as u64,
                &hash(bytes),
            )
            .await
            .unwrap();
        self.store
            .upload(&self.admin, &self.submission, &ticket.transfer_id, bytes)
            .await
            .unwrap();
        self.store
            .confirm_input(&self.admin, &self.submission, &ticket.object_id)
            .await
            .unwrap()
    }
    fn actor(&self) -> AttemptOwner {
        let d = DispatchOwner::authenticated("issuer-a", &uid(), &uid()).unwrap();
        AttemptOwner::authenticated(&d, &uid(), &uid(), &uid(), 1).unwrap()
    }
}
#[test]
fn canonical_manifest_preserves_chinese_and_rejects_case_collisions() {
    let obj = ObjectRef {
        object_id: "00000000-0000-0000-0000-000000000001".into(),
        display_name: "输入.json".into(),
        size: CONTENT.len() as u64,
        sha256: hash(CONTENT),
    };
    let m = manifest::build(
        "00000000-0000-0000-0000-000000000010",
        vec![obj.clone()],
        vec![],
    )
    .unwrap();
    assert_eq!(m.schema_version, 1);
    assert_eq!(
        hash(CONTENT),
        "da36d1f53c22016dc9eeb7414d1be41ddca4b70f4bf1abd0deca0eecfe9ac355"
    );
    assert_eq!(
        m.digest,
        "bdeea31f87e4f58622b59528ec72503adf196e2c26adcdd25468ba0440cf1099"
    );
    assert_eq!(
        serde_json::to_value(&m).unwrap()["inputs"][0]["displayName"],
        "输入.json"
    );
    let mut duplicate = obj.clone();
    duplicate.object_id = "00000000-0000-0000-0000-000000000002".into();
    assert!(manifest::build(&m.manifest_id, vec![obj, duplicate], vec![]).is_err());
}
#[tokio::test]
async fn confirmed_staged_object_cannot_be_read_as_an_attempt_input() {
    let f = Fixture::new().await;
    let obj = f.input("input.json", CONTENT).await;
    assert_eq!(obj.sha256, hash(CONTENT));
    assert!(f
        .store
        .open_input(&f.actor(), &obj.object_id)
        .await
        .is_err());
}
#[tokio::test]
async fn equal_hash_never_grants_another_owner_access() {
    let f = Fixture::new().await;
    let obj = f.input("input.json", CONTENT).await;
    let other = Administrator::authenticated("admin-b").unwrap();
    assert!(f
        .store
        .confirm_input(&other, &f.submission, &obj.object_id)
        .await
        .is_err());
}

impl Fixture {
    async fn attached(&self, outputs: &[&str]) -> (DispatchOwner, AttemptOwner, ExchangeManifest) {
        let object = self.input("input.json", CONTENT).await;
        let m = self
            .store
            .prepare_manifest(
                &self.admin,
                &self.submission,
                &[object.object_id],
                &outputs.iter().map(|n| n.to_string()).collect::<Vec<_>>(),
            )
            .await
            .unwrap();
        let dispatch = DispatchOwner::authenticated("issuer-a", &uid(), &uid()).unwrap();
        self.store
            .attach(&self.admin, &dispatch, &self.submission, &m.manifest_id)
            .await
            .unwrap();
        let actor = AttemptOwner::authenticated(&dispatch, &uid(), &uid(), &uid(), 1).unwrap();
        (dispatch, actor, m)
    }
}
#[tokio::test]
async fn upload_confirmation_replay_is_immutable_and_rejects_incomplete_bytes() {
    let f = Fixture::new().await;
    let t = f
        .store
        .prepare_input(
            &f.admin,
            &f.submission,
            "input.json",
            CONTENT.len() as u64,
            &hash(CONTENT),
        )
        .await
        .unwrap();
    for bytes in [&CONTENT[..CONTENT.len() - 1], b"different".as_slice()] {
        assert!(f
            .store
            .upload(&f.admin, &f.submission, &t.transfer_id, bytes)
            .await
            .is_err());
        assert!(f
            .store
            .confirm_input(&f.admin, &f.submission, &t.object_id)
            .await
            .is_err());
    }
    let first = f
        .store
        .upload(&f.admin, &f.submission, &t.transfer_id, CONTENT)
        .await
        .unwrap();
    let again = f
        .store
        .upload(&f.admin, &f.submission, &t.transfer_id, CONTENT)
        .await
        .unwrap();
    assert_eq!(first, again);
    assert_eq!(
        first,
        f.store
            .confirm_input(&f.admin, &f.submission, &t.object_id)
            .await
            .unwrap()
    );
    assert_eq!(
        first,
        f.store
            .confirm_input(&f.admin, &f.submission, &t.object_id)
            .await
            .unwrap()
    );
    let t2 = f
        .store
        .prepare_input(
            &f.admin,
            &f.submission,
            "input.json",
            CONTENT.len() as u64,
            &hash(CONTENT),
        )
        .await
        .unwrap();
    assert_eq!(t.object_id, t2.object_id);
    assert_eq!(
        std::fs::read_dir(f._dir.path().join("exchange/blobs"))
            .unwrap()
            .count(),
        1,
        "replays must not grow persistent blob storage"
    );

    assert!(f
        .store
        .upload(
            &f.admin,
            &f.submission,
            &t.transfer_id,
            b"different".as_slice()
        )
        .await
        .is_err());
}
#[tokio::test]
async fn attachment_is_atomic_and_cannot_switch_issuer_branch_or_submission() {
    use sea_orm::TransactionTrait;
    let f = Fixture::new().await;
    let object = f.input("input.json", CONTENT).await;
    let m = f
        .store
        .prepare_manifest(
            &f.admin,
            &f.submission,
            &[object.object_id],
            &["output.txt".into()],
        )
        .await
        .unwrap();
    let dispatch = DispatchOwner::authenticated("issuer-a", &uid(), &uid()).unwrap();
    let tx = f.db.begin().await.unwrap();
    f.store
        .attach_in(&tx, &f.admin, &dispatch, &f.submission, &m.manifest_id)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    f.store
        .attach(&f.admin, &dispatch, &f.submission, &m.manifest_id)
        .await
        .unwrap();
    f.store
        .attach(&f.admin, &dispatch, &f.submission, &m.manifest_id)
        .await
        .unwrap();
    for wrong in [
        DispatchOwner::authenticated("issuer-b", &dispatch.dispatch, &dispatch.branch).unwrap(),
        DispatchOwner::authenticated("issuer-a", &dispatch.dispatch, &uid()).unwrap(),
        DispatchOwner::authenticated("issuer-a", &uid(), &dispatch.branch).unwrap(),
    ] {
        assert!(f
            .store
            .attach(&f.admin, &wrong, &f.submission, &m.manifest_id)
            .await
            .is_err());
    }
    assert!(f
        .store
        .prepare_input(&f.admin, &f.submission, "late.txt", 0, &hash(b""))
        .await
        .is_err());
    let actor = AttemptOwner::authenticated(&dispatch, &uid(), &uid(), &uid(), 1).unwrap();
    assert!(f
        .store
        .open_input(&actor, &m.inputs[0].object_id)
        .await
        .is_err());
}
#[tokio::test]
async fn concurrent_reservations_obey_two_gib_total_without_large_files() {
    let f = Fixture::new().await;
    let store = &f.store;
    let admin = &f.admin;
    let submission = &f.submission;
    let results = futures::future::join_all((0..5).map(|i| async move {
        store
            .prepare_input(
                admin,
                submission,
                &format!("file-{i}"),
                MAX_FILE_BYTES,
                &hash(CONTENT),
            )
            .await
    }))
    .await;
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 4);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(ExchangeError::Quota)))
            .count(),
        1
    );
    assert!(matches!(
        store
            .prepare_input(admin, &uid(), "big", MAX_FILE_BYTES + 1, &hash(CONTENT))
            .await,
        Err(ExchangeError::Quota)
    ));
}
#[test]
fn canonical_order_and_limits_are_independent_of_input_order() {
    let a = ObjectRef {
        object_id: "00000000-0000-0000-0000-000000000001".into(),
        display_name: "A.json".into(),
        size: 1,
        sha256: hash(b"a"),
    };
    let b = ObjectRef {
        object_id: "00000000-0000-0000-0000-000000000002".into(),
        display_name: "B.json".into(),
        size: 1,
        sha256: hash(b"b"),
    };
    let mid = uid();
    let one = manifest::build(&mid, vec![a.clone(), b.clone()], vec![]).unwrap();
    let two = manifest::build(&mid, vec![b.clone(), a.clone()], vec![]).unwrap();
    assert_eq!(one, two);
    let mut altered = b.clone();
    altered.size = 2;
    assert_ne!(
        one.digest,
        manifest::build(&mid, vec![a.clone(), altered], vec![])
            .unwrap()
            .digest
    );
    let mut collision = b.clone();
    collision.display_name = "a.JSON".into();
    assert!(manifest::build(&mid, vec![a.clone(), collision], vec![]).is_err());
    assert!(manifest::build(&mid, vec![a.clone(), a], vec![]).is_err());
    let huge = ObjectRef {
        size: MAX_FILE_BYTES + 1,
        ..b
    };
    assert!(matches!(
        manifest::build(&mid, vec![huge], vec![]),
        Err(ExchangeError::Quota)
    ));
    let outputs = (0..201)
        .map(|i| OutputSlot {
            slot_id: uid(),
            display_name: format!("out{i}"),
        })
        .collect();
    assert!(matches!(
        manifest::build(&mid, vec![], outputs),
        Err(ExchangeError::Quota)
    ));
    for name in [
        "../x",
        "/absolute",
        "C:\\x",
        "\\\\server\\share",
        "\\\\?\\C:\\x",
        "file:stream",
        "CON.txt",
        "NUL",
        "COM1",
        "LPT9.txt",
        "a.",
        "a ",
        ".",
        "..",
    ] {
        assert!(manifest::validate_name(name).is_err(), "{name}");
    }
}
#[test]
fn bounded_copy_accepts_exact_file_limit_and_rejects_one_more_byte() {
    let mut exact = std::io::Read::take(std::io::repeat(0), 536_870_912);
    assert_eq!(
        super::files::copy_bounded(&mut exact, &mut std::io::sink(), u64::MAX)
            .unwrap()
            .0,
        MAX_FILE_BYTES
    );
    let mut excessive = std::io::Read::take(std::io::repeat(0), 536_870_913);
    assert!(matches!(
        super::files::copy_bounded(&mut excessive, &mut std::io::sink(), u64::MAX),
        Err(ExchangeError::Quota)
    ));
}
#[tokio::test]
async fn materialization_and_sealing_preserve_bytes_and_exit_uncertainty() {
    use std::io::Read;
    for verified in [false, true] {
        let f = Fixture::new().await;
        let (dispatch, actor, m) = f.attached(&["output.txt"]).await;
        let paths = f.store.materialize(&actor).await.unwrap();
        assert_eq!(
            std::fs::read(&paths.inputs[&m.inputs[0].object_id]).unwrap(),
            CONTENT
        );
        assert!(std::fs::metadata(&paths.inputs[&m.inputs[0].object_id])
            .unwrap()
            .permissions()
            .readonly());
        assert_eq!(
            f.store.materialize(&actor).await.unwrap().outputs,
            paths.outputs
        );
        std::fs::write(&paths.outputs[&m.outputs[0].slot_id], CONTENT).unwrap();
        let proof = if verified {
            ExitVerification::Verified
        } else {
            ExitVerification::Unverified
        };
        let artifact = f
            .store
            .seal(&actor, &m.outputs[0].slot_id, proof)
            .await
            .unwrap();
        assert_eq!(artifact.exit_verified, verified);
        assert_eq!(artifact.object.sha256, hash(CONTENT));
        let wire = serde_json::to_value(&artifact).unwrap();
        assert_eq!(wire["objectId"], artifact.object.object_id);
        assert!(wire.get("object").is_none());
        assert_eq!(
            serde_json::from_value::<ArtifactRef>(wire).unwrap(),
            artifact
        );
        std::fs::write(&paths.outputs[&m.outputs[0].slot_id], b"later changes").unwrap();
        let mut bytes = Vec::new();
        f.store
            .open_artifact(&dispatch, &artifact.object.object_id)
            .await
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, CONTENT);
        assert_eq!(
            f.store
                .seal(&actor, &m.outputs[0].slot_id, proof)
                .await
                .unwrap(),
            artifact
        );
    }
}
#[tokio::test]
async fn advancing_attempt_revokes_old_writes_without_revoking_historical_reads() {
    use std::io::Read;
    let f = Fixture::new().await;
    let (dispatch, first, m) = f.attached(&["output.txt"]).await;
    let p = f.store.materialize(&first).await.unwrap();
    std::fs::write(&p.outputs[&m.outputs[0].slot_id], CONTENT).unwrap();
    let old = f
        .store
        .seal(&first, &m.outputs[0].slot_id, ExitVerification::Unverified)
        .await
        .unwrap();
    let next = AttemptOwner::authenticated(&dispatch, &first.task, &uid(), &uid(), 2).unwrap();
    let new = f.store.materialize(&next).await.unwrap();
    assert_ne!(new.outputs, p.outputs);
    assert!(matches!(
        f.store
            .seal(&first, &m.outputs[0].slot_id, ExitVerification::Verified)
            .await,
        Err(ExchangeError::Forbidden)
    ));
    assert!(f.store.materialize(&first).await.is_err());
    let unseen_old =
        AttemptOwner::authenticated(&dispatch, &first.task, &uid(), &uid(), 1).unwrap();
    assert!(f.store.materialize(&unseen_old).await.is_err());
    let mut bytes = Vec::new();
    f.store
        .open_input(&first, &m.inputs[0].object_id)
        .await
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, CONTENT);
    assert!(f
        .store
        .open_input(&next, &old.object.object_id)
        .await
        .is_err());
}
#[tokio::test]
async fn revocation_between_copy_and_commit_cannot_publish_an_artifact() {
    let f = Fixture::new().await;
    let (dispatch, actor, m) = f.attached(&["output.txt"]).await;
    let paths = f.store.materialize(&actor).await.unwrap();
    let output = &paths.outputs[&m.outputs[0].slot_id];
    std::fs::write(output, CONTENT).unwrap();
    let prepared = f
        .store
        .prepare_seal(&actor, &m.outputs[0].slot_id, ExitVerification::Unverified)
        .await
        .unwrap();
    let copied = f.store.copy_seal(prepared).await.unwrap();
    f.store.revoke_attempt(&actor).await.unwrap();
    f.store.revoke_attempt(&actor).await.unwrap();
    assert!(matches!(
        f.store.commit_seal(copied).await,
        Err(ExchangeError::Forbidden)
    ));
    assert!(f
        .store
        .sealed_artifacts(&dispatch)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(std::fs::read(output).unwrap(), CONTENT);
}
#[cfg(unix)]
#[tokio::test]
async fn link_and_file_replacements_between_open_and_copy_never_publish() {
    use std::os::unix::fs::symlink;
    for mode in ["symlink", "hardlink", "changed"] {
        let f = Fixture::new().await;
        let (dispatch, actor, m) = f.attached(&["output.txt"]).await;
        let paths = f.store.materialize(&actor).await.unwrap();
        let output = &paths.outputs[&m.outputs[0].slot_id];
        std::fs::write(output, CONTENT).unwrap();
        let prepared = f
            .store
            .prepare_seal(&actor, &m.outputs[0].slot_id, ExitVerification::Unverified)
            .await
            .unwrap();
        let elsewhere = f._dir.path().join("untouched");
        std::fs::write(&elsewhere, b"private").unwrap();
        match mode {
            "symlink" => {
                std::fs::rename(output, output.with_extension("saved")).unwrap();
                symlink(&elsewhere, output).unwrap();
            }
            "hardlink" => std::fs::hard_link(output, output.with_extension("link")).unwrap(),
            _ => std::fs::write(output, b"changed content").unwrap(),
        }
        assert!(f.store.copy_seal(prepared).await.is_err());
        assert!(f
            .store
            .sealed_artifacts(&dispatch)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(std::fs::read(&elsewhere).unwrap(), b"private");
    }
}
#[cfg(unix)]
#[tokio::test]
async fn preexisting_links_and_linked_output_directories_are_rejected() {
    use std::os::unix::fs::symlink;
    for mode in ["file_symlink", "file_hardlink", "directory_symlink"] {
        let f = Fixture::new().await;
        let (_dispatch, actor, m) = f.attached(&["output.txt"]).await;
        let paths = f.store.materialize(&actor).await.unwrap();
        let output = &paths.outputs[&m.outputs[0].slot_id];
        let elsewhere = f._dir.path().join("outside");
        std::fs::create_dir(&elsewhere).unwrap();
        let file = elsewhere.join("source");
        std::fs::write(&file, CONTENT).unwrap();
        match mode {
            "file_symlink" => symlink(&file, output).unwrap(),
            "file_hardlink" => std::fs::hard_link(&file, output).unwrap(),
            _ => {
                std::fs::remove_dir(output.parent().unwrap()).unwrap();
                symlink(&elsewhere, output.parent().unwrap()).unwrap();
            }
        }
        assert!(f
            .store
            .seal(&actor, &m.outputs[0].slot_id, ExitVerification::Unverified)
            .await
            .is_err());
        assert_eq!(std::fs::read(&file).unwrap(), CONTENT);
    }
}
#[tokio::test]
async fn exchange_migration_preserves_existing_records_and_refuses_destructive_down() {
    use sea_orm_migration::{MigrationTrait, MigratorTrait, SchemaManager};
    let f = Fixture::new().await;
    crate::db::service::app_metadata_service::upsert_value(
        &f.db,
        "exchange-legacy-sentinel",
        "retained",
    )
    .await
    .unwrap();
    crate::db::migration::Migrator::down(&f.db, Some(1))
        .await
        .unwrap();
    crate::db::migration::Migrator::up(&f.db, None)
        .await
        .unwrap();
    crate::db::migration::Migrator::up(&f.db, None)
        .await
        .unwrap();
    assert_eq!(
        crate::db::service::app_metadata_service::get_value(&f.db, "exchange-legacy-sentinel")
            .await
            .unwrap()
            .as_deref(),
        Some("retained")
    );
    f.input("input.json", CONTENT).await;
    assert!(super::migration::Migration
        .down(&SchemaManager::new(&f.db))
        .await
        .is_err());
}

#[tokio::test]
async fn authenticated_dispatch_input_and_collection_preserve_wire_ids_and_hashes() {
    use sea_orm::TransactionTrait;
    use std::io::Read;
    let hq = Fixture::new().await;
    let (dispatch, actor, manifest) = hq.attached(&["output.txt"]).await;
    let branch = Fixture::new().await;
    let tx = branch.db.begin().await.unwrap();
    branch
        .store
        .accept_dispatch_in(&tx, &dispatch, &hq.submission, &manifest)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let transfer = branch
        .store
        .prepare_dispatch_input(&dispatch, &manifest.inputs[0].object_id)
        .await
        .unwrap();
    let received = branch
        .store
        .receive_dispatch_input(transfer, CONTENT)
        .await
        .unwrap();
    let tx = branch.db.begin().await.unwrap();
    branch
        .store
        .commit_dispatch_input_in(&tx, received)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let paths = branch.store.materialize(&actor).await.unwrap();
    std::fs::write(&paths.outputs[&manifest.outputs[0].slot_id], CONTENT).unwrap();
    let artifact = branch
        .store
        .seal(
            &actor,
            &manifest.outputs[0].slot_id,
            ExitVerification::Unverified,
        )
        .await
        .unwrap();
    let received = hq
        .store
        .receive_artifact(&hq.admin, &actor, &artifact, CONTENT)
        .await
        .unwrap();
    let tx = hq.db.begin().await.unwrap();
    hq.store.commit_collection_in(&tx, received).await.unwrap();
    tx.rollback().await.unwrap();
    assert!(hq
        .store
        .collected_artifacts(&hq.admin, &dispatch.dispatch)
        .await
        .unwrap()
        .is_empty());
    let received = hq
        .store
        .receive_artifact(&hq.admin, &actor, &artifact, CONTENT)
        .await
        .unwrap();
    let tx = hq.db.begin().await.unwrap();
    assert_eq!(
        hq.store.commit_collection_in(&tx, received).await.unwrap(),
        artifact
    );
    tx.commit().await.unwrap();
    let mut bytes = Vec::new();
    hq.store
        .open_collected(&hq.admin, &artifact.object.object_id)
        .await
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, CONTENT);
    let received = hq
        .store
        .receive_artifact(&hq.admin, &actor, &artifact, CONTENT)
        .await
        .unwrap();
    let tx = hq.db.begin().await.unwrap();
    assert_eq!(
        hq.store.commit_collection_in(&tx, received).await.unwrap(),
        artifact
    );
    tx.commit().await.unwrap();
    assert_eq!(
        hq.store
            .collected_artifacts(&hq.admin, &dispatch.dispatch)
            .await
            .unwrap()
            .len(),
        1
    );
    let wrong = Administrator::authenticated("someone-else").unwrap();
    assert!(hq
        .store
        .open_collected(&wrong, &artifact.object.object_id)
        .await
        .is_err());
}
#[tokio::test]
async fn revoked_dispatch_cannot_commit_bytes_received_before_revocation() {
    use sea_orm::TransactionTrait;
    let hq = Fixture::new().await;
    let (dispatch, actor, manifest) = hq.attached(&["output.txt"]).await;
    let branch = Fixture::new().await;
    let tx = branch.db.begin().await.unwrap();
    branch
        .store
        .accept_dispatch_in(&tx, &dispatch, &hq.submission, &manifest)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let permit = branch
        .store
        .prepare_dispatch_input(&dispatch, &manifest.inputs[0].object_id)
        .await
        .unwrap();
    let received = branch
        .store
        .receive_dispatch_input(permit, CONTENT)
        .await
        .unwrap();
    let tx = branch.db.begin().await.unwrap();
    branch
        .store
        .revoke_dispatch_in(&tx, &dispatch)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let tx = branch.db.begin().await.unwrap();
    assert!(matches!(
        branch.store.commit_dispatch_input_in(&tx, received).await,
        Err(ExchangeError::Forbidden)
    ));
    tx.rollback().await.unwrap();
    assert!(branch.store.materialize(&actor).await.is_err());
}

#[cfg(unix)]
#[test]
fn changes_during_the_actual_copy_are_rejected_from_open_handles() {
    use std::io::Write;
    use std::os::unix::fs::symlink;
    struct Mutator<F: FnMut()> {
        action: Option<F>,
    }
    impl<F: FnMut()> Write for Mutator<F> {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            if let Some(mut action) = self.action.take() {
                action();
            }
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    for mode in ["contents", "link", "directory"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let source_dir = root.join("source");
        std::fs::create_dir(&source_dir).unwrap();
        let source_path = source_dir.join("result.txt");
        std::fs::write(&source_path, CONTENT).unwrap();
        let outside = root.join("outside.txt");
        std::fs::write(&outside, b"untouched").unwrap();
        let directory = super::files::Directory::absolute(&source_dir).unwrap();
        let mut source = super::files::Source::open(directory, "result.txt").unwrap();
        let mut writer = Mutator {
            action: Some(|| match mode {
                "contents" => std::fs::write(&source_path, b"mutated").unwrap(),
                "link" => {
                    std::fs::rename(&source_path, source_path.with_extension("saved")).unwrap();
                    symlink(&outside, &source_path).unwrap();
                }
                _ => {
                    std::fs::rename(&source_dir, root.join("old-source")).unwrap();
                    symlink(&root, &source_dir).unwrap();
                }
            }),
        };
        assert!(source.copy(&mut writer, MAX_FILE_BYTES).is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"untouched");
    }
}
#[tokio::test]
async fn an_attempt_revoked_before_materialization_cannot_be_resurrected() {
    let f = Fixture::new().await;
    let (_dispatch, actor, _m) = f.attached(&["output.txt"]).await;
    f.store.revoke_attempt(&actor).await.unwrap();
    f.store.revoke_attempt(&actor).await.unwrap();
    assert!(matches!(
        f.store.materialize(&actor).await,
        Err(ExchangeError::Forbidden)
    ));
}
#[tokio::test]
async fn abandoned_temporary_bytes_are_not_confirmed_after_store_reopen() {
    let f = Fixture::new().await;
    let ticket = f
        .store
        .prepare_input(
            &f.admin,
            &f.submission,
            "incomplete.txt",
            CONTENT.len() as u64,
            &hash(CONTENT),
        )
        .await
        .unwrap();
    std::fs::write(
        f._dir.path().join("exchange/temporary").join(uid()),
        CONTENT,
    )
    .unwrap();
    #[cfg(unix)]
    let root = f._dir.path().canonicalize().unwrap().join("exchange");
    #[cfg(windows)]
    let root = f._dir.path().join("exchange");
    let reopened = Exchange::open(f.db.clone(), &root).unwrap();
    assert!(reopened
        .confirm_input(&f.admin, &f.submission, &ticket.object_id)
        .await
        .is_err());
}
#[cfg(windows)]
#[tokio::test]
async fn windows_reparse_directories_and_device_names_are_rejected() {
    let f = Fixture::new().await;
    let (_dispatch, actor, m) = f.attached(&["output.txt"]).await;
    let paths = f.store.materialize(&actor).await.unwrap();
    let output = &paths.outputs[&m.outputs[0].slot_id];
    let outside = f._dir.path().join("junction-target");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("output.txt"), CONTENT).unwrap();
    std::fs::remove_dir(output.parent().unwrap()).unwrap();
    junction::create(&outside, output.parent().unwrap()).unwrap();
    assert!(f
        .store
        .seal(&actor, &m.outputs[0].slot_id, ExitVerification::Unverified)
        .await
        .is_err());
    for name in [
        "CON",
        "NUL.txt",
        "COM¹",
        "COM2.txt",
        "LPT³",
        "file:stream",
        "\\\\server\\share",
        "\\\\?\\C:\\device",
    ] {
        assert!(manifest::validate_name(name).is_err());
    }
    for path in ["\\\\server\\share", "\\\\?\\C:\\device", "\\\\.\\NUL"] {
        assert!(super::files::Directory::absolute(std::path::Path::new(path)).is_err());
    }
}

#[tokio::test]
async fn equal_bytes_with_distinct_grants_remain_isolated_after_materialization() {
    use std::io::Read;
    let f = Fixture::new().await;
    let a = f.input("same.json", CONTENT).await;
    let b = Administrator::authenticated("admin-b").unwrap();
    let sub = uid();
    let ticket = f
        .store
        .prepare_input(&b, &sub, "same.json", CONTENT.len() as u64, &hash(CONTENT))
        .await
        .unwrap();
    f.store
        .upload(&b, &sub, &ticket.transfer_id, CONTENT)
        .await
        .unwrap();
    let b_object = f
        .store
        .confirm_input(&b, &sub, &ticket.object_id)
        .await
        .unwrap();
    assert_eq!(a.sha256, b_object.sha256);
    assert_ne!(a.object_id, b_object.object_id);
    assert!(f
        .store
        .prepare_manifest(&b, &sub, std::slice::from_ref(&a.object_id), &[])
        .await
        .is_err());
    let manifest = f
        .store
        .prepare_manifest(&b, &sub, std::slice::from_ref(&b_object.object_id), &[])
        .await
        .unwrap();
    let owner = DispatchOwner::authenticated("issuer-b", &uid(), &uid()).unwrap();
    f.store
        .attach(&b, &owner, &sub, &manifest.manifest_id)
        .await
        .unwrap();
    let actor = AttemptOwner::authenticated(&owner, &uid(), &uid(), &uid(), 1).unwrap();
    f.store.materialize(&actor).await.unwrap();
    assert!(matches!(
        f.store.open_input(&actor, &a.object_id).await,
        Err(ExchangeError::Forbidden)
    ));
    let mut bytes = Vec::new();
    f.store
        .open_input(&actor, &b_object.object_id)
        .await
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, CONTENT);
}
#[test]
fn canonical_manifest_enforces_literal_wire_and_total_limits() {
    let objects: Vec<_> = (0..5)
        .map(|i| ObjectRef {
            object_id: uid(),
            display_name: format!("item{i}"),
            size: 536_870_912,
            sha256: hash(b"metadata"),
        })
        .collect();
    assert!(manifest::build(&uid(), objects[..4].to_vec(), vec![]).is_ok());
    assert!(matches!(
        manifest::build(&uid(), objects, vec![]),
        Err(ExchangeError::Quota)
    ));
    let outputs: Vec<_> = (0..200)
        .map(|i| OutputSlot {
            slot_id: uid(),
            display_name: format!("slot{i}"),
        })
        .collect();
    assert!(manifest::build(&uid(), vec![], outputs).is_ok());
    let long: Vec<_> = (0..200)
        .map(|i| ObjectRef {
            object_id: uid(),
            display_name: format!("{}-{i:03}", "x".repeat(230)),
            size: 1,
            sha256: hash(b"a"),
        })
        .collect();
    assert!(matches!(
        manifest::build(&uid(), long, vec![]),
        Err(ExchangeError::Quota)
    ));
    for name in ["COM¹", "COM².txt", "LPT³", "CON .txt"] {
        assert!(manifest::validate_name(name).is_err());
    }
}
#[tokio::test]
async fn concurrent_output_reservations_cannot_overcommit_the_manifest_budget() {
    let f = Fixture::new().await;
    let (_dispatch, actor, m) = f.attached(&["one", "two", "three", "four"]).await;
    let paths = f.store.materialize(&actor).await.unwrap();
    // Sparse metadata only: no 512 MiB payload is written or copied here.
    for path in paths.outputs.values() {
        std::fs::File::create(path)
            .unwrap()
            .set_len(536_870_912)
            .unwrap();
    }
    let results = futures::future::join_all(m.outputs.iter().map(|s| {
        f.store
            .prepare_seal(&actor, &s.slot_id, ExitVerification::Unverified)
    }))
    .await;
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 3);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(ExchangeError::Quota)))
            .count(),
        1
    );
    f.store.revoke_attempt(&actor).await.unwrap();
}

#[tokio::test]
async fn revoked_attempt_cannot_receive_or_commit_a_collected_artifact() {
    use sea_orm::TransactionTrait;
    for revoke_before_receive in [false, true] {
        let hq = Fixture::new().await;
        let (_, actor, manifest) = hq.attached(&["output.txt"]).await;
        let artifact = ArtifactRef {
            object: ObjectRef {
                object_id: uid(),
                display_name: manifest.outputs[0].display_name.clone(),
                size: CONTENT.len() as u64,
                sha256: hash(CONTENT),
            },
            slot_id: manifest.outputs[0].slot_id.clone(),
            dispatch_id: actor.dispatch.dispatch.clone(),
            branch_id: actor.dispatch.branch.clone(),
            task_id: actor.task.clone(),
            attempt_id: actor.attempt.clone(),
            run_id: actor.run.clone(),
            sealed_at: chrono::Utc::now(),
            exit_verified: false,
        };
        if revoke_before_receive {
            hq.store.revoke_attempt(&actor).await.unwrap();
            assert!(matches!(
                hq.store
                    .receive_artifact(&hq.admin, &actor, &artifact, CONTENT)
                    .await,
                Err(ExchangeError::Forbidden)
            ));
        } else {
            let received = hq
                .store
                .receive_artifact(&hq.admin, &actor, &artifact, CONTENT)
                .await
                .unwrap();
            hq.store.revoke_attempt(&actor).await.unwrap();
            let tx = hq.db.begin().await.unwrap();
            let result = hq.store.commit_collection_in(&tx, received).await;
            tx.rollback().await.unwrap();
            assert!(matches!(result, Err(ExchangeError::Forbidden)));
        }
        assert!(hq
            .store
            .collected_artifacts(&hq.admin, &actor.dispatch.dispatch)
            .await
            .unwrap()
            .is_empty());
    }
}
