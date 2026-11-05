use super::{
    files::{Layout, Source, Temporary},
    manifest,
    types::*,
};
use chrono::Utc;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, FromQueryResult,
    Statement, TransactionTrait,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub struct Exchange {
    db: DatabaseConnection,
    files: Arc<Layout>,
}
#[derive(FromQueryResult)]
struct Input {
    object_id: String,
    submission_id: String,
    display_name: String,
    size: i64,
    sha256: String,
    transfer_id: String,
    blob_id: Option<String>,
    confirmed: bool,
}
impl Input {
    fn reference(&self) -> Result<ObjectRef> {
        Ok(ObjectRef {
            object_id: self.object_id.clone(),
            display_name: self.display_name.clone(),
            size: u64::try_from(self.size).map_err(|_| ExchangeError::Storage)?,
            sha256: self.sha256.clone(),
        })
    }
}
enum InputKey<'a> {
    Object(&'a str),
    Transfer(&'a str),
    Name(&'a str),
}
async fn input<C: ConnectionTrait>(
    db: &C,
    submission: &str,
    key: InputKey<'_>,
) -> Result<Option<Input>> {
    let (column, value) = match key {
        InputKey::Object(v) => ("object_id", v),
        InputKey::Transfer(v) => ("transfer_id", v),
        InputKey::Name(v) => ("name_key", v),
    };
    Input::find_by_statement(Statement::from_sql_and_values(DbBackend::Sqlite,
        format!("SELECT object_id,submission_id,display_name,size,sha256,transfer_id,blob_id,confirmed FROM hall_exchange_input WHERE submission_id=? AND {column}=?"),
        vec![submission.to_owned().into(),value.to_owned().into()])).one(db).await.map_err(Into::into)
}
async fn owns<C: ConnectionTrait>(db: &C, admin: &Administrator, submission: &str) -> Result<()> {
    if db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT 1 FROM hall_exchange_submission WHERE submission_id=? AND owner_id=?",
            vec![submission.to_owned().into(), admin.owner.clone().into()],
        ))
        .await?
        .is_none()
    {
        return Err(ExchangeError::Forbidden);
    }
    Ok(())
}
async fn lock_submission(
    tx: &DatabaseTransaction,
    admin: &Administrator,
    submission: &str,
    create: bool,
) -> Result<()> {
    uuid(submission)?;
    if create {
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"INSERT INTO hall_exchange_submission(submission_id,owner_id) VALUES(?,?) ON CONFLICT(submission_id) DO NOTHING",vec![submission.to_owned().into(),admin.owner.clone().into()])).await?;
    }
    if tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_exchange_submission SET owner_id=owner_id WHERE submission_id=? AND owner_id=?",vec![submission.to_owned().into(),admin.owner.clone().into()])).await?.rows_affected()!=1 {return Err(ExchangeError::Forbidden);}
    Ok(())
}
async fn frozen<C: ConnectionTrait>(db: &C, submission: &str) -> Result<bool> {
    Ok(db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT 1 FROM hall_exchange_attachment WHERE submission_id=?",
            vec![submission.to_owned().into()],
        ))
        .await?
        .is_some())
}
async fn persist_blob<C: ConnectionTrait>(db: &C, blob: &str, size: u64, hash: &str) -> Result<()> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO hall_exchange_blob(blob_id,size,sha256) VALUES(?,?,?)",
        vec![
            blob.to_owned().into(),
            (size as i64).into(),
            hash.to_owned().into(),
        ],
    ))
    .await?;
    Ok(())
}
async fn load_manifest<C: ConnectionTrait>(db: &C, id: &str) -> Result<(String, ExchangeManifest)> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT submission_id,manifest_json FROM hall_exchange_manifest WHERE manifest_id=?",
            vec![id.to_owned().into()],
        ))
        .await?
        .ok_or(ExchangeError::Forbidden)?;
    let manifest: ExchangeManifest =
        serde_json::from_str(&row.try_get::<String>("", "manifest_json")?)
            .map_err(|_| ExchangeError::Storage)?;
    manifest::verify(&manifest).map_err(|_| ExchangeError::Storage)?;
    Ok((row.try_get("", "submission_id")?, manifest))
}
#[derive(FromQueryResult)]
struct Attachment {
    submission_id: String,
    issuer_id: String,
    branch_id: String,
    manifest_id: String,
    task_id: Option<String>,
    current_run_id: Option<String>,
    generation: i64,
    revoked: bool,
}
async fn attachment<C: ConnectionTrait>(db: &C, actor: &DispatchOwner) -> Result<Attachment> {
    let row = Attachment::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT * FROM hall_exchange_attachment WHERE dispatch_id=?",
        vec![actor.dispatch.clone().into()],
    ))
    .one(db)
    .await?
    .ok_or(ExchangeError::Forbidden)?;
    if row.issuer_id != actor.issuer || row.branch_id != actor.branch {
        return Err(ExchangeError::Forbidden);
    }
    Ok(row)
}
#[derive(FromQueryResult)]
struct Attempt {
    attempt_id: String,
    task_id: String,
    dispatch_id: String,
    generation: i64,
    nonce: String,
    state: String,
    revoked: bool,
}
async fn attempt<C: ConnectionTrait>(
    db: &C,
    actor: &AttemptOwner,
    writing: bool,
) -> Result<(Attachment, Attempt)> {
    let attached = attachment(db, &actor.dispatch).await?;
    let row = Attempt::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT * FROM hall_exchange_attempt WHERE run_id=?",
        vec![actor.run.clone().into()],
    ))
    .one(db)
    .await?
    .ok_or(ExchangeError::Forbidden)?;
    if row.attempt_id != actor.attempt
        || row.task_id != actor.task
        || row.dispatch_id != actor.dispatch.dispatch
        || row.generation != actor.generation
        || (writing
            && (attached.revoked
                || row.revoked
                || attached.current_run_id.as_deref() != Some(&actor.run)
                || attached.generation != actor.generation))
    {
        return Err(ExchangeError::Forbidden);
    }
    Ok((attached, row))
}
impl Exchange {
    /// Once per host store; clone the service for concurrent operations. Root
    /// must be an absolute local path with existing, non-linked ancestors.
    pub fn open(db: DatabaseConnection, root: &Path) -> Result<Self> {
        Ok(Self {
            db,
            files: Arc::new(Layout::open(root)?),
        })
    }
    pub async fn prepare_input(
        &self,
        admin: &Administrator,
        submission: &str,
        name: &str,
        size: u64,
        hash: &str,
    ) -> Result<UploadTicket> {
        let key = manifest::name_key(name)?;
        manifest::validate_hash(hash)?;
        if size > MAX_FILE_BYTES {
            return Err(ExchangeError::Quota);
        }
        let tx = self.db.begin().await?;
        lock_submission(&tx, admin, submission, true).await?;
        if let Some(existing) = input(&tx, submission, InputKey::Name(&key)).await? {
            if existing.display_name != name
                || existing.size != size as i64
                || existing.sha256 != hash
            {
                return Err(ExchangeError::Conflict);
            }
            tx.commit().await?;
            return Ok(UploadTicket {
                object_id: existing.object_id,
                transfer_id: existing.transfer_id,
            });
        }
        if frozen(&tx, submission).await? {
            return Err(ExchangeError::Forbidden);
        }
        let quota=tx.query_one(Statement::from_sql_and_values(DbBackend::Sqlite,"SELECT COUNT(*) AS items,COALESCE(SUM(size),0) AS bytes FROM hall_exchange_input WHERE submission_id=?",vec![submission.to_owned().into()])).await?.ok_or(ExchangeError::Storage)?;
        if quota.try_get::<i64>("", "items")? >= MAX_MANIFEST_ITEMS as i64
            || quota.try_get::<i64>("", "bytes")? as u64 + size > MAX_TOTAL_BYTES
        {
            return Err(ExchangeError::Quota);
        }
        let object_id = id();
        let transfer_id = id();
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"INSERT INTO hall_exchange_input(object_id,submission_id,display_name,name_key,size,sha256,transfer_id) VALUES(?,?,?,?,?,?,?)",vec![object_id.clone().into(),submission.to_owned().into(),name.to_owned().into(),key.into(),(size as i64).into(),hash.to_owned().into(),transfer_id.clone().into()])).await?;
        tx.commit().await?;
        Ok(UploadTicket {
            object_id,
            transfer_id,
        })
    }
    async fn receive<R: AsyncRead + Unpin + Send>(
        &self,
        mut reader: R,
        size: u64,
        hash: &str,
    ) -> Result<Temporary> {
        if size > MAX_FILE_BYTES {
            return Err(ExchangeError::Quota);
        }
        manifest::validate_hash(hash)?;
        let temporary = self.files.temporary()?;
        let mut file = tokio::fs::File::from_std(
            temporary
                .file
                .as_ref()
                .ok_or(ExchangeError::Storage)?
                .try_clone()?,
        );
        let mut count = 0u64;
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            count = count.checked_add(n as u64).ok_or(ExchangeError::Quota)?;
            if count > size {
                return Err(ExchangeError::Conflict);
            }
            digest.update(&buffer[..n]);
            file.write_all(&buffer[..n]).await?;
        }
        if count != size || format!("{:x}", digest.finalize()) != hash {
            return Err(ExchangeError::Conflict);
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        Ok(temporary)
    }
    async fn publish_temporary(&self, temporary: Temporary, size: u64) -> Result<String> {
        let blobs = self.files.blobs.clone();
        tokio::task::spawn_blocking(move || temporary.publish(&blobs, size))
            .await
            .map_err(|_| ExchangeError::Storage)?
    }

    pub async fn upload<R: AsyncRead + Unpin + Send>(
        &self,
        admin: &Administrator,
        submission: &str,
        transfer: &str,
        reader: R,
    ) -> Result<ObjectRef> {
        uuid(submission)?;
        uuid(transfer)?;
        owns(&self.db, admin, submission).await?;
        let planned = input(&self.db, submission, InputKey::Transfer(transfer))
            .await?
            .ok_or(ExchangeError::Forbidden)?;
        let reference = planned.reference()?;
        let temporary = self
            .receive(reader, reference.size, &reference.sha256)
            .await?;
        let tx = self.db.begin().await?;
        lock_submission(&tx, admin, submission, false).await?;
        let current = input(&tx, submission, InputKey::Transfer(transfer))
            .await?
            .ok_or(ExchangeError::Forbidden)?;
        if current.reference()? != reference {
            return Err(ExchangeError::Conflict);
        }
        if current.blob_id.is_none() {
            if frozen(&tx, submission).await? {
                return Err(ExchangeError::Forbidden);
            }
            let blob = self.publish_temporary(temporary, reference.size).await?;
            persist_blob(&tx, &blob, reference.size, &reference.sha256).await?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_exchange_input SET blob_id=? WHERE object_id=? AND blob_id IS NULL",
                vec![blob.into(), current.object_id.into()],
            ))
            .await?;
        }
        tx.commit().await?;
        Ok(reference)
    }
    pub async fn confirm_input(
        &self,
        admin: &Administrator,
        submission: &str,
        object: &str,
    ) -> Result<ObjectRef> {
        uuid(submission)?;
        uuid(object)?;
        owns(&self.db, admin, submission).await?;
        let planned = input(&self.db, submission, InputKey::Object(object))
            .await?
            .ok_or(ExchangeError::Forbidden)?;
        let blob = planned.blob_id.clone().ok_or(ExchangeError::Conflict)?;
        let reference = planned.reference()?;
        let size = reference.size;
        let hash = reference.sha256.clone();
        let files = self.files.clone();
        drop(
            tokio::task::spawn_blocking(move || files.blob(&blob, size, &hash))
                .await
                .map_err(|_| ExchangeError::Storage)??,
        );
        let tx = self.db.begin().await?;
        lock_submission(&tx, admin, submission, false).await?;
        let current = input(&tx, submission, InputKey::Object(object))
            .await?
            .ok_or(ExchangeError::Forbidden)?;
        if current.reference()? != reference || current.blob_id != planned.blob_id {
            return Err(ExchangeError::Conflict);
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_exchange_input SET confirmed=1 WHERE object_id=?",
            vec![object.to_owned().into()],
        ))
        .await?;
        tx.commit().await?;
        Ok(reference)
    }
    pub async fn prepare_manifest(
        &self,
        admin: &Administrator,
        submission: &str,
        objects: &[String],
        output_names: &[String],
    ) -> Result<ExchangeManifest> {
        let mut objects = objects.to_vec();
        objects.sort();
        if objects.windows(2).any(|w| w[0] == w[1]) {
            return Err(ExchangeError::Invalid);
        }
        let mut names = output_names.to_vec();
        names.sort();
        for object in &objects {
            uuid(object)?;
        }
        for name in &names {
            manifest::validate_name(name)?;
        }
        if objects.len() + names.len() > MAX_MANIFEST_ITEMS {
            return Err(ExchangeError::Quota);
        }
        let selection =
            serde_json::to_string(&(&objects, &names)).map_err(|_| ExchangeError::Invalid)?;
        let tx = self.db.begin().await?;
        lock_submission(&tx, admin, submission, true).await?;
        if let Some(row)=tx.query_one(Statement::from_sql_and_values(DbBackend::Sqlite,"SELECT manifest_id FROM hall_exchange_manifest WHERE submission_id=? AND selection_json=?",vec![submission.to_owned().into(),selection.clone().into()])).await? {
            let (_,manifest)=load_manifest(&tx,&row.try_get::<String>("","manifest_id")?).await?;tx.commit().await?;return Ok(manifest);
        }
        if frozen(&tx, submission).await? {
            return Err(ExchangeError::Forbidden);
        }
        let mut inputs = Vec::new();
        for object in &objects {
            let row = input(&tx, submission, InputKey::Object(object))
                .await?
                .ok_or(ExchangeError::Forbidden)?;
            if !row.confirmed || row.blob_id.is_none() {
                return Err(ExchangeError::Conflict);
            }
            inputs.push(row.reference()?);
        }
        let outputs = names
            .into_iter()
            .map(|display_name| OutputSlot {
                slot_id: id(),
                display_name,
            })
            .collect();
        let manifest = manifest::build(&id(), inputs, outputs)?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"INSERT INTO hall_exchange_manifest(manifest_id,submission_id,manifest_json,digest,selection_json) VALUES(?,?,?,?,?)",vec![manifest.manifest_id.clone().into(),submission.to_owned().into(),serde_json::to_string(&manifest).map_err(|_|ExchangeError::Storage)?.into(),manifest.digest.clone().into(),selection.into()])).await?;
        tx.commit().await?;
        Ok(manifest)
    }
    /// Compose with dispatch creation in the caller's SAME database transaction.
    pub async fn attach_in(
        &self,
        tx: &DatabaseTransaction,
        admin: &Administrator,
        owner: &DispatchOwner,
        submission: &str,
        manifest_id: &str,
    ) -> Result<ExchangeManifest> {
        lock_submission(tx, admin, submission, false).await?;
        let (actual, manifest) = load_manifest(tx, manifest_id).await?;
        if actual != submission {
            return Err(ExchangeError::Forbidden);
        }
        if let Some(row)=tx.query_one(Statement::from_sql_and_values(DbBackend::Sqlite,"SELECT dispatch_id,issuer_id,branch_id,manifest_id FROM hall_exchange_attachment WHERE submission_id=? OR dispatch_id=?",vec![submission.to_owned().into(),owner.dispatch.clone().into()])).await? {
            if row.try_get::<String>("","dispatch_id")?!=owner.dispatch || row.try_get::<String>("","issuer_id")?!=owner.issuer || row.try_get::<String>("","branch_id")?!=owner.branch || row.try_get::<String>("","manifest_id")?!=manifest_id {return Err(ExchangeError::Conflict);}
            return Ok(manifest);
        }
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"INSERT INTO hall_exchange_attachment(dispatch_id,submission_id,issuer_id,branch_id,manifest_id) VALUES(?,?,?,?,?)",vec![owner.dispatch.clone().into(),submission.to_owned().into(),owner.issuer.clone().into(),owner.branch.clone().into(),manifest_id.to_owned().into()])).await?;
        Ok(manifest)
    }
    pub async fn attach(
        &self,
        admin: &Administrator,
        owner: &DispatchOwner,
        submission: &str,
        manifest_id: &str,
    ) -> Result<ExchangeManifest> {
        let tx = self.db.begin().await?;
        let manifest = self
            .attach_in(&tx, admin, owner, submission, manifest_id)
            .await?;
        tx.commit().await?;
        Ok(manifest)
    }
    pub async fn list_inputs(&self, actor: &AttemptOwner) -> Result<Vec<ObjectRef>> {
        let (attached, run) = attempt(&self.db, actor, false).await?;
        if run.state != "ready" {
            return Err(ExchangeError::Forbidden);
        }
        Ok(load_manifest(&self.db, &attached.manifest_id)
            .await?
            .1
            .inputs)
    }
    pub async fn open_input(&self, actor: &AttemptOwner, object: &str) -> Result<File> {
        uuid(object)?;
        let (attached, run) = attempt(&self.db, actor, false).await?;
        if run.state != "ready" {
            return Err(ExchangeError::Forbidden);
        }
        let manifest = load_manifest(&self.db, &attached.manifest_id).await?.1;
        let reference = manifest
            .inputs
            .into_iter()
            .find(|r| r.object_id == object)
            .ok_or(ExchangeError::Forbidden)?;
        let row = input(&self.db, &attached.submission_id, InputKey::Object(object))
            .await?
            .ok_or(ExchangeError::Forbidden)?;
        let blob = row.blob_id.ok_or(ExchangeError::Forbidden)?;
        let files = self.files.clone();
        tokio::task::spawn_blocking(move || files.blob(&blob, reference.size, &reference.sha256))
            .await
            .map_err(|_| ExchangeError::Storage)?
    }
}

/// Local paths are host output only, never accepted back as model authority.
pub struct MaterializedAttempt {
    pub inputs: BTreeMap<String, PathBuf>,
    pub outputs: BTreeMap<String, PathBuf>,
}
pub struct SealPreparation {
    actor: AttemptOwner,
    slot: String,
    state: SealState,
}
enum SealState {
    Existing(ArtifactRef),
    Copy {
        source: Source,
        nonce: String,
        verification: ExitVerification,
    },
}
pub struct PreparedArtifact {
    actor: AttemptOwner,
    slot: String,
    nonce: Option<String>,
    temporary: Option<Temporary>,
    artifact: ArtifactRef,
}
#[derive(FromQueryResult)]
struct Slot {
    display_name: String,
    nonce: Option<String>,
    reserved_size: i64,
    blob_id: Option<String>,
    object_id: Option<String>,
    artifact_json: Option<String>,
}
impl Slot {
    fn artifact(&self) -> Result<Option<ArtifactRef>> {
        let Some(json) = &self.artifact_json else {
            if self.blob_id.is_some() || self.object_id.is_some() {
                return Err(ExchangeError::Storage);
            }
            return Ok(None);
        };
        let artifact: ArtifactRef =
            serde_json::from_str(json).map_err(|_| ExchangeError::Storage)?;
        if self.blob_id.is_none()
            || self.object_id.as_deref() != Some(&artifact.object.object_id)
            || self.reserved_size as u64 != artifact.object.size
            || artifact.object.display_name != self.display_name
        {
            return Err(ExchangeError::Storage);
        }
        Ok(Some(artifact))
    }
}
async fn slot<C: ConnectionTrait>(db: &C, run: &str, slot: &str) -> Result<Slot> {
    Slot::find_by_statement(Statement::from_sql_and_values(DbBackend::Sqlite,"SELECT display_name,nonce,reserved_size,blob_id,object_id,artifact_json FROM hall_exchange_slot WHERE run_id=? AND slot_id=?",vec![run.to_owned().into(),slot.to_owned().into()])).one(db).await?.ok_or(ExchangeError::Forbidden)
}
async fn lock_dispatch(tx: &DatabaseTransaction, owner: &DispatchOwner) -> Result<Attachment> {
    if tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_exchange_attachment SET generation=generation WHERE dispatch_id=? AND issuer_id=? AND branch_id=? AND revoked=0",vec![owner.dispatch.clone().into(),owner.issuer.clone().into(),owner.branch.clone().into()])).await?.rows_affected()!=1 {return Err(ExchangeError::Forbidden);}
    attachment(tx, owner).await
}
async fn attached_manifest<C: ConnectionTrait>(
    db: &C,
    row: &Attachment,
) -> Result<ExchangeManifest> {
    let (submission, manifest) = load_manifest(db, &row.manifest_id).await?;
    if submission != row.submission_id {
        return Err(ExchangeError::Storage);
    }
    Ok(manifest)
}
async fn advance(
    tx: &DatabaseTransaction,
    attached: &Attachment,
    actor: &AttemptOwner,
) -> Result<()> {
    if attached
        .task_id
        .as_ref()
        .is_some_and(|task| task != &actor.task)
        || actor.generation < attached.generation
        || (actor.generation == attached.generation
            && attached
                .current_run_id
                .as_ref()
                .is_some_and(|run| run != &actor.run))
    {
        return Err(ExchangeError::Forbidden);
    }
    if actor.generation > attached.generation {
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_exchange_attempt SET revoked=1 WHERE dispatch_id=?",
            vec![actor.dispatch.dispatch.clone().into()],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_exchange_slot SET nonce=NULL,reserved_size=0 WHERE blob_id IS NULL AND run_id IN(SELECT run_id FROM hall_exchange_attempt WHERE dispatch_id=?)",vec![actor.dispatch.dispatch.clone().into()])).await?;
    }
    tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_exchange_attachment SET task_id=COALESCE(task_id,?),current_run_id=?,generation=? WHERE dispatch_id=?",vec![actor.task.clone().into(),actor.run.clone().into(),actor.generation.into(),actor.dispatch.dispatch.clone().into()])).await?;
    Ok(())
}
impl Exchange {
    pub async fn materialize(&self, actor: &AttemptOwner) -> Result<MaterializedAttempt> {
        let tx = self.db.begin().await?;
        let attached = lock_dispatch(&tx, &actor.dispatch).await?;
        let manifest = attached_manifest(&tx, &attached).await?;
        let existing = Attempt::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT * FROM hall_exchange_attempt WHERE run_id=?",
            vec![actor.run.clone().into()],
        ))
        .one(&tx)
        .await?;
        if let Some(row) = &existing {
            if row.revoked
                || row.attempt_id != actor.attempt
                || row.task_id != actor.task
                || row.dispatch_id != actor.dispatch.dispatch
                || row.generation != actor.generation
            {
                return Err(ExchangeError::Forbidden);
            }
        }
        let mut objects = Vec::new();
        for reference in &manifest.inputs {
            let input = input(
                &tx,
                &attached.submission_id,
                InputKey::Object(&reference.object_id),
            )
            .await?
            .ok_or(ExchangeError::Forbidden)?;
            if !input.confirmed || input.reference()? != *reference {
                return Err(ExchangeError::Conflict);
            }
            objects.push((
                reference.clone(),
                input.blob_id.ok_or(ExchangeError::Conflict)?,
            ));
        }
        advance(&tx, &attached, actor).await?;
        if let Some(row) = existing.as_ref().filter(|r| r.state == "ready") {
            let files = self.files.clone();
            let nonce = row.nonce.clone();
            tx.commit().await?;
            return tokio::task::spawn_blocking(move || files.existing_paths(&nonce, &manifest))
                .await
                .map_err(|_| ExchangeError::Storage)?;
        }
        let nonce = id();
        if existing.is_some() {
            tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_exchange_attempt SET nonce=? WHERE run_id=? AND state='preparing' AND revoked=0",vec![nonce.clone().into(),actor.run.clone().into()])).await?;
        } else {
            tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"INSERT INTO hall_exchange_attempt(run_id,attempt_id,task_id,dispatch_id,generation,nonce,state) VALUES(?,?,?,?,?,?,'preparing')",vec![actor.run.clone().into(),actor.attempt.clone().into(),actor.task.clone().into(),actor.dispatch.dispatch.clone().into(),actor.generation.into(),nonce.clone().into()])).await?;
        }
        tx.commit().await?;
        let files = self.files.clone();
        let directory_nonce = nonce.clone();
        let outputs = manifest.outputs.clone();
        let paths = tokio::task::spawn_blocking(move || -> Result<MaterializedAttempt> {
            let directory = files.attempt_directory(&directory_nonce)?;
            let mut inputs = BTreeMap::new();
            for (object, blob) in &objects {
                inputs.insert(
                    object.object_id.clone(),
                    files.input_copy(&directory, object, blob)?,
                );
            }
            let outputs = files.prepare_outputs(&directory, &outputs)?;
            directory.validate()?;
            Ok(MaterializedAttempt { inputs, outputs })
        })
        .await
        .map_err(|_| ExchangeError::Storage)??;
        let tx = self.db.begin().await?;
        lock_dispatch(&tx, &actor.dispatch).await?;
        let (_, current) = attempt(&tx, actor, true).await?;
        if current.nonce != nonce || current.state != "preparing" {
            return Err(ExchangeError::Conflict);
        }
        for output in &manifest.outputs {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "INSERT INTO hall_exchange_slot(run_id,slot_id,display_name) VALUES(?,?,?)",
                vec![
                    actor.run.clone().into(),
                    output.slot_id.clone().into(),
                    output.display_name.clone().into(),
                ],
            ))
            .await?;
        }
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_exchange_attempt SET state='ready' WHERE run_id=? AND nonce=? AND revoked=0",vec![actor.run.clone().into(),nonce.into()])).await?;
        tx.commit().await?;
        Ok(paths)
    }
    pub async fn prepare_seal(
        &self,
        actor: &AttemptOwner,
        slot_id: &str,
        verification: ExitVerification,
    ) -> Result<SealPreparation> {
        uuid(slot_id)?;
        let (attached, run) = attempt(&self.db, actor, true).await?;
        if run.state != "ready" {
            return Err(ExchangeError::Forbidden);
        }
        let manifest = attached_manifest(&self.db, &attached).await?;
        let output = manifest
            .outputs
            .iter()
            .find(|s| s.slot_id == slot_id)
            .ok_or(ExchangeError::Forbidden)?;
        let existing = slot(&self.db, &actor.run, slot_id).await?;
        if let Some(artifact) = existing.artifact()? {
            return Ok(SealPreparation {
                actor: actor.clone(),
                slot: slot_id.into(),
                state: SealState::Existing(artifact),
            });
        }
        let directory = self.files.output_directory(&run.nonce, slot_id)?;
        let source = Source::open(directory, &output.display_name)?;
        let size = source.len();
        if size > MAX_FILE_BYTES {
            return Err(ExchangeError::Quota);
        }
        let tx = self.db.begin().await?;
        lock_dispatch(&tx, &actor.dispatch).await?;
        let (_, current) = attempt(&tx, actor, true).await?;
        if current.state != "ready" || current.nonce != run.nonce {
            return Err(ExchangeError::Forbidden);
        }
        let current_slot = slot(&tx, &actor.run, slot_id).await?;
        if let Some(artifact) = current_slot.artifact()? {
            tx.commit().await?;
            return Ok(SealPreparation {
                actor: actor.clone(),
                slot: slot_id.into(),
                state: SealState::Existing(artifact),
            });
        }
        if current_slot.display_name != output.display_name {
            return Err(ExchangeError::Storage);
        }
        let used=tx.query_one(Statement::from_sql_and_values(DbBackend::Sqlite,"SELECT COALESCE(SUM(reserved_size),0) AS bytes FROM hall_exchange_slot WHERE run_id=? AND slot_id<>?",vec![actor.run.clone().into(),slot_id.to_owned().into()])).await?.ok_or(ExchangeError::Storage)?.try_get::<i64>("","bytes")? as u64;
        let input_bytes: u64 = manifest.inputs.iter().map(|i| i.size).sum();
        if input_bytes + used + size > MAX_TOTAL_BYTES {
            return Err(ExchangeError::Quota);
        }
        let nonce = id();
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_exchange_slot SET nonce=?,reserved_size=? WHERE run_id=? AND slot_id=? AND blob_id IS NULL",vec![nonce.clone().into(),(size as i64).into(),actor.run.clone().into(),slot_id.to_owned().into()])).await?;
        tx.commit().await?;
        Ok(SealPreparation {
            actor: actor.clone(),
            slot: slot_id.into(),
            state: SealState::Copy {
                source,
                nonce,
                verification,
            },
        })
    }
    pub async fn copy_seal(&self, prepared: SealPreparation) -> Result<PreparedArtifact> {
        attempt(&self.db, &prepared.actor, true).await?;
        let files = self.files.clone();
        tokio::task::spawn_blocking(move || {
            let SealPreparation { actor, slot, state } = prepared;
            match state {
                SealState::Existing(artifact) => Ok(PreparedArtifact {
                    actor,
                    slot,
                    nonce: None,
                    temporary: None,
                    artifact,
                }),
                SealState::Copy {
                    mut source,
                    nonce,
                    verification,
                } => {
                    let mut temporary = files.temporary()?;
                    let (size, sha256) = source.copy(
                        temporary.file.as_mut().ok_or(ExchangeError::Storage)?,
                        source.len(),
                    )?;
                    temporary
                        .file
                        .as_ref()
                        .ok_or(ExchangeError::Storage)?
                        .sync_all()?;
                    let artifact = ArtifactRef {
                        object: ObjectRef {
                            object_id: id(),
                            display_name: source.name().into(),
                            size,
                            sha256,
                        },
                        slot_id: slot.clone(),
                        dispatch_id: actor.dispatch.dispatch.clone(),
                        branch_id: actor.dispatch.branch.clone(),
                        task_id: actor.task.clone(),
                        attempt_id: actor.attempt.clone(),
                        run_id: actor.run.clone(),
                        sealed_at: Utc::now(),
                        exit_verified: matches!(verification, ExitVerification::Verified),
                    };
                    Ok(PreparedArtifact {
                        actor,
                        slot,
                        nonce: Some(nonce),
                        temporary: Some(temporary),
                        artifact,
                    })
                }
            }
        })
        .await
        .map_err(|_| ExchangeError::Storage)?
    }
    /// Caller may recheck issuer credential revocation in this same transaction.
    pub async fn commit_seal_in(
        &self,
        tx: &DatabaseTransaction,
        prepared: PreparedArtifact,
    ) -> Result<ArtifactRef> {
        lock_dispatch(tx, &prepared.actor.dispatch).await?;
        let (_, run) = attempt(tx, &prepared.actor, true).await?;
        if run.state != "ready" {
            return Err(ExchangeError::Forbidden);
        }
        let current = slot(tx, &prepared.actor.run, &prepared.slot).await?;
        if let Some(artifact) = current.artifact()? {
            if prepared.nonce.is_none() && artifact == prepared.artifact {
                return Ok(artifact);
            }
            return Err(ExchangeError::Conflict);
        }
        if current.nonce != prepared.nonce
            || prepared.nonce.is_none()
            || current.reserved_size as u64 != prepared.artifact.object.size
        {
            return Err(ExchangeError::Conflict);
        }
        let temporary = prepared.temporary.ok_or(ExchangeError::Storage)?;
        let blob = self
            .publish_temporary(temporary, prepared.artifact.object.size)
            .await?;
        persist_blob(
            tx,
            &blob,
            prepared.artifact.object.size,
            &prepared.artifact.object.sha256,
        )
        .await?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_exchange_slot SET nonce=NULL,blob_id=?,object_id=?,artifact_json=? WHERE run_id=? AND slot_id=?",vec![blob.into(),prepared.artifact.object.object_id.clone().into(),serde_json::to_string(&prepared.artifact).map_err(|_|ExchangeError::Storage)?.into(),prepared.actor.run.into(),prepared.slot.into()])).await?;
        Ok(prepared.artifact)
    }
    pub async fn commit_seal(&self, prepared: PreparedArtifact) -> Result<ArtifactRef> {
        let actor = prepared.actor.clone();
        let slot = prepared.slot.clone();
        let nonce = prepared.nonce.clone();
        let tx = self.db.begin().await?;
        let result = match self.commit_seal_in(&tx, prepared).await {
            Ok(artifact) => tx.commit().await.map(|_| artifact).map_err(Into::into),
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        };
        if result.is_err() {
            if let Some(nonce) = nonce {
                let _ = self.release_reservation(&actor, &slot, &nonce).await;
            }
        }
        result
    }
    async fn release_reservation(
        &self,
        actor: &AttemptOwner,
        slot: &str,
        nonce: &str,
    ) -> Result<()> {
        self.db.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_exchange_slot SET nonce=NULL,reserved_size=0 WHERE run_id=? AND slot_id=? AND nonce=? AND blob_id IS NULL",vec![actor.run.clone().into(),slot.to_owned().into(),nonce.to_owned().into()])).await?;
        Ok(())
    }
    pub async fn seal(
        &self,
        actor: &AttemptOwner,
        slot: &str,
        verification: ExitVerification,
    ) -> Result<ArtifactRef> {
        let prepared = self.prepare_seal(actor, slot, verification).await?;
        let nonce = match &prepared.state {
            SealState::Copy { nonce, .. } => Some(nonce.clone()),
            SealState::Existing(_) => None,
        };
        let staged = match self.copy_seal(prepared).await {
            Ok(p) => p,
            Err(e) => {
                if let Some(nonce) = nonce {
                    let _ = self.release_reservation(actor, slot, &nonce).await;
                }
                return Err(e);
            }
        };
        self.commit_seal(staged).await
    }
    pub async fn cancel_pending(&self, actor: &AttemptOwner, slot: &str) -> Result<()> {
        let tx = self.db.begin().await?;
        lock_dispatch(&tx, &actor.dispatch).await?;
        attempt(&tx, actor, true).await?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_exchange_slot SET nonce=NULL,reserved_size=0 WHERE run_id=? AND slot_id=? AND blob_id IS NULL",vec![actor.run.clone().into(),slot.to_owned().into()])).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Coordinate with the runtime's revocation in the SAME transaction in 05.
    pub async fn revoke_attempt_in(
        &self,
        tx: &DatabaseTransaction,
        actor: &AttemptOwner,
    ) -> Result<()> {
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_exchange_attachment SET generation=generation WHERE dispatch_id=? AND issuer_id=? AND branch_id=?",vec![actor.dispatch.dispatch.clone().into(),actor.dispatch.issuer.clone().into(),actor.dispatch.branch.clone().into()])).await?;
        let attached = attachment(tx, &actor.dispatch).await?;
        if attached.task_id.as_ref().is_some_and(|t| t != &actor.task) {
            return Err(ExchangeError::Forbidden);
        }
        let existing = Attempt::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT * FROM hall_exchange_attempt WHERE run_id=?",
            vec![actor.run.clone().into()],
        ))
        .one(tx)
        .await?;
        if existing.is_some() {
            attempt(tx, actor, false).await?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_exchange_attempt SET revoked=1 WHERE run_id=?",
                vec![actor.run.clone().into()],
            ))
            .await?;
        } else if actor.generation >= attached.generation {
            if actor.generation == attached.generation
                && attached
                    .current_run_id
                    .as_ref()
                    .is_some_and(|r| r != &actor.run)
            {
                return Err(ExchangeError::Forbidden);
            }
            tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"INSERT INTO hall_exchange_attempt(run_id,attempt_id,task_id,dispatch_id,generation,nonce,state,revoked) VALUES(?,?,?,?,?,?,'preparing',1)",vec![actor.run.clone().into(),actor.attempt.clone().into(),actor.task.clone().into(),actor.dispatch.dispatch.clone().into(),actor.generation.into(),id().into()])).await?;
        }
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_exchange_slot SET nonce=NULL,reserved_size=0 WHERE run_id=? AND blob_id IS NULL",vec![actor.run.clone().into()])).await?;
        Ok(())
    }
    pub async fn revoke_attempt(&self, actor: &AttemptOwner) -> Result<()> {
        let tx = self.db.begin().await?;
        self.revoke_attempt_in(&tx, actor).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn revoke_dispatch_in(
        &self,
        tx: &DatabaseTransaction,
        owner: &DispatchOwner,
    ) -> Result<()> {
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_exchange_attachment SET revoked=1 WHERE dispatch_id=? AND issuer_id=? AND branch_id=?",vec![owner.dispatch.clone().into(),owner.issuer.clone().into(),owner.branch.clone().into()])).await?;
        attachment(tx, owner).await?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE hall_exchange_attempt SET revoked=1 WHERE dispatch_id=?",
            vec![owner.dispatch.clone().into()],
        ))
        .await?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"UPDATE hall_exchange_slot SET nonce=NULL,reserved_size=0 WHERE blob_id IS NULL AND run_id IN(SELECT run_id FROM hall_exchange_attempt WHERE dispatch_id=?)",vec![owner.dispatch.clone().into()])).await?;
        Ok(())
    }
    pub async fn sealed_artifacts(&self, owner: &DispatchOwner) -> Result<Vec<ArtifactRef>> {
        attachment(&self.db, owner).await?;
        let rows=self.db.query_all(Statement::from_sql_and_values(DbBackend::Sqlite,"SELECT s.artifact_json FROM hall_exchange_slot s JOIN hall_exchange_attempt a ON a.run_id=s.run_id WHERE a.dispatch_id=? AND s.blob_id IS NOT NULL ORDER BY a.generation,s.slot_id",vec![owner.dispatch.clone().into()])).await?;
        rows.into_iter()
            .map(|r| {
                serde_json::from_str(&r.try_get::<String>("", "artifact_json")?)
                    .map_err(|_| ExchangeError::Storage)
            })
            .collect()
    }
    pub async fn open_artifact(&self, owner: &DispatchOwner, object: &str) -> Result<File> {
        uuid(object)?;
        attachment(&self.db, owner).await?;
        let row=self.db.query_one(Statement::from_sql_and_values(DbBackend::Sqlite,"SELECT s.blob_id,s.artifact_json FROM hall_exchange_slot s JOIN hall_exchange_attempt a ON a.run_id=s.run_id WHERE a.dispatch_id=? AND s.object_id=?",vec![owner.dispatch.clone().into(),object.to_owned().into()])).await?.ok_or(ExchangeError::Forbidden)?;
        let artifact: ArtifactRef =
            serde_json::from_str(&row.try_get::<String>("", "artifact_json")?)
                .map_err(|_| ExchangeError::Storage)?;
        let blob: String = row.try_get("", "blob_id")?;
        let files = self.files.clone();
        tokio::task::spawn_blocking(move || {
            files.blob(&blob, artifact.object.size, &artifact.object.sha256)
        })
        .await
        .map_err(|_| ExchangeError::Storage)?
    }
}

/// Private transfer permits carry resource scope, not a general file capability.
pub struct DispatchInput {
    owner: DispatchOwner,
    submission: String,
    object: ObjectRef,
}
pub struct ReceivedInput {
    permit: DispatchInput,
    temporary: Temporary,
}
pub struct ReceivedArtifact {
    admin: Administrator,
    actor: AttemptOwner,
    artifact: ArtifactRef,
    temporary: Temporary,
}
impl Exchange {
    pub async fn open_uploaded(
        &self,
        admin: &Administrator,
        submission: &str,
        object: &str,
    ) -> Result<File> {
        owns(&self.db, admin, submission).await?;
        let row = input(&self.db, submission, InputKey::Object(object))
            .await?
            .ok_or(ExchangeError::Forbidden)?;
        if !row.confirmed {
            return Err(ExchangeError::Conflict);
        }
        let blob = row.blob_id.clone().ok_or(ExchangeError::Conflict)?;
        let reference = row.reference()?;
        let files = self.files.clone();
        tokio::task::spawn_blocking(move || files.blob(&blob, reference.size, &reference.sha256))
            .await
            .map_err(|_| ExchangeError::Storage)?
    }
    /// Register authenticated inbound DATA, never a deserialized authority.
    /// The protocol adapter checks live issuer trust in this same transaction.
    pub async fn accept_dispatch_in(
        &self,
        tx: &DatabaseTransaction,
        owner: &DispatchOwner,
        submission: &str,
        incoming: &ExchangeManifest,
    ) -> Result<()> {
        manifest::verify(incoming)?;
        let incoming = manifest::build(
            &incoming.manifest_id,
            incoming.inputs.clone(),
            incoming.outputs.clone(),
        )?;
        let remote = Administrator::authenticated(&format!(
            "issuer:{:x}",
            Sha256::digest(owner.issuer.as_bytes())
        ))?;
        lock_submission(tx, &remote, submission, true).await?;
        if let Some(row) = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT manifest_id FROM hall_exchange_attachment WHERE dispatch_id=?",
                vec![owner.dispatch.clone().into()],
            ))
            .await?
        {
            let attached = attachment(tx, owner).await?;
            if attached.revoked
                || row.try_get::<String>("", "manifest_id")? != incoming.manifest_id
                || attached_manifest(tx, &attached).await? != incoming
            {
                return Err(ExchangeError::Conflict);
            }
            return Ok(());
        }
        if frozen(tx, submission).await? {
            return Err(ExchangeError::Conflict);
        }
        for object in &incoming.inputs {
            if let Some(existing)=Input::find_by_statement(Statement::from_sql_and_values(DbBackend::Sqlite,"SELECT object_id,submission_id,display_name,size,sha256,transfer_id,blob_id,confirmed FROM hall_exchange_input WHERE object_id=?",vec![object.object_id.clone().into()])).one(tx).await? {
                if existing.submission_id!=submission || existing.reference()?!=*object {return Err(ExchangeError::Forbidden);}
            } else {
                tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"INSERT INTO hall_exchange_input(object_id,submission_id,display_name,name_key,size,sha256,transfer_id) VALUES(?,?,?,?,?,?,?)",vec![object.object_id.clone().into(),submission.to_owned().into(),object.display_name.clone().into(),manifest::name_key(&object.display_name)?.into(),(object.size as i64).into(),object.sha256.clone().into(),id().into()])).await?;
            }
        }
        if let Some(row) = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT submission_id FROM hall_exchange_manifest WHERE manifest_id=?",
                vec![incoming.manifest_id.clone().into()],
            ))
            .await?
        {
            if row.try_get::<String>("", "submission_id")? != submission
                || load_manifest(tx, &incoming.manifest_id).await?.1 != incoming
            {
                return Err(ExchangeError::Conflict);
            }
        } else {
            let mut ids: Vec<_> = incoming
                .inputs
                .iter()
                .map(|i| i.object_id.clone())
                .collect();
            ids.sort();
            let mut names: Vec<_> = incoming
                .outputs
                .iter()
                .map(|s| s.display_name.clone())
                .collect();
            names.sort();
            tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"INSERT INTO hall_exchange_manifest(manifest_id,submission_id,manifest_json,digest,selection_json) VALUES(?,?,?,?,?)",vec![incoming.manifest_id.clone().into(),submission.to_owned().into(),serde_json::to_string(&incoming).map_err(|_|ExchangeError::Storage)?.into(),incoming.digest.clone().into(),serde_json::to_string(&(ids,names)).map_err(|_|ExchangeError::Storage)?.into()])).await?;
        }
        self.attach_in(tx, &remote, owner, submission, &incoming.manifest_id)
            .await?;
        Ok(())
    }
    pub async fn prepare_dispatch_input(
        &self,
        owner: &DispatchOwner,
        object: &str,
    ) -> Result<DispatchInput> {
        uuid(object)?;
        let attached = attachment(&self.db, owner).await?;
        if attached.revoked {
            return Err(ExchangeError::Forbidden);
        }
        let manifest = attached_manifest(&self.db, &attached).await?;
        let reference = manifest
            .inputs
            .into_iter()
            .find(|i| i.object_id == object)
            .ok_or(ExchangeError::Forbidden)?;
        Ok(DispatchInput {
            owner: owner.clone(),
            submission: attached.submission_id,
            object: reference,
        })
    }
    pub async fn receive_dispatch_input<R: AsyncRead + Unpin + Send>(
        &self,
        permit: DispatchInput,
        reader: R,
    ) -> Result<ReceivedInput> {
        let current = self
            .prepare_dispatch_input(&permit.owner, &permit.object.object_id)
            .await?;
        if current.submission != permit.submission || current.object != permit.object {
            return Err(ExchangeError::Forbidden);
        }
        let temporary = self
            .receive(reader, permit.object.size, &permit.object.sha256)
            .await?;
        Ok(ReceivedInput { permit, temporary })
    }
    pub async fn commit_dispatch_input_in(
        &self,
        tx: &DatabaseTransaction,
        received: ReceivedInput,
    ) -> Result<ObjectRef> {
        let attached = lock_dispatch(tx, &received.permit.owner).await?;
        let reference = &received.permit.object;
        let manifest = attached_manifest(tx, &attached).await?;
        if attached.submission_id != received.permit.submission
            || !manifest.inputs.contains(reference)
        {
            return Err(ExchangeError::Forbidden);
        }
        let current = input(
            tx,
            &attached.submission_id,
            InputKey::Object(&reference.object_id),
        )
        .await?
        .ok_or(ExchangeError::Forbidden)?;
        if current.reference()? != *reference {
            return Err(ExchangeError::Conflict);
        }
        if current.blob_id.is_none() {
            let blob = self
                .publish_temporary(received.temporary, reference.size)
                .await?;
            persist_blob(tx, &blob, reference.size, &reference.sha256).await?;
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_exchange_input SET blob_id=?,confirmed=1 WHERE object_id=?",
                vec![blob.into(), reference.object_id.clone().into()],
            ))
            .await?;
        } else {
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE hall_exchange_input SET confirmed=1 WHERE object_id=?",
                vec![reference.object_id.clone().into()],
            ))
            .await?;
        }
        Ok(received.permit.object)
    }
    async fn validate_collection<C: ConnectionTrait>(
        &self,
        db: &C,
        admin: &Administrator,
        actor: &AttemptOwner,
        artifact: &ArtifactRef,
    ) -> Result<Attachment> {
        let attached = attachment(db, &actor.dispatch).await?;
        owns(db, admin, &attached.submission_id).await?;
        // HQ can collect without local materialization, but a runtime revocation
        // may have created an attempt tombstone. Check it both before streaming
        // and in the final collection transaction so that receipt cannot revive it.
        if db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT 1 FROM hall_exchange_attempt WHERE run_id=?",
                vec![actor.run.clone().into()],
            ))
            .await?
            .is_some()
        {
            attempt(db, actor, true).await?;
        }
        if attached.revoked
            || artifact.dispatch_id != actor.dispatch.dispatch
            || artifact.branch_id != actor.dispatch.branch
            || artifact.task_id != actor.task
            || artifact.attempt_id != actor.attempt
            || artifact.run_id != actor.run
        {
            return Err(ExchangeError::Forbidden);
        }
        uuid(&artifact.object.object_id)?;
        manifest::validate_hash(&artifact.object.sha256)?;
        if artifact.object.size > MAX_FILE_BYTES {
            return Err(ExchangeError::Quota);
        }
        let manifest = attached_manifest(db, &attached).await?;
        if !manifest.outputs.iter().any(|s| {
            s.slot_id == artifact.slot_id && s.display_name == artifact.object.display_name
        }) {
            return Err(ExchangeError::Forbidden);
        }
        Ok(attached)
    }
    pub async fn receive_artifact<R: AsyncRead + Unpin + Send>(
        &self,
        admin: &Administrator,
        actor: &AttemptOwner,
        artifact: &ArtifactRef,
        reader: R,
    ) -> Result<ReceivedArtifact> {
        let tx = self.db.begin().await?;
        lock_dispatch(&tx, &actor.dispatch).await?;
        let attached = self
            .validate_collection(&tx, admin, actor, artifact)
            .await?;
        advance(&tx, &attached, actor).await?;
        tx.commit().await?;
        let temporary = self
            .receive(reader, artifact.object.size, &artifact.object.sha256)
            .await?;
        Ok(ReceivedArtifact {
            admin: admin.clone(),
            actor: actor.clone(),
            artifact: artifact.clone(),
            temporary,
        })
    }
    /// Recheck the authenticated remote issuer in this same transaction in 05.
    /// Collection never writes any task execution status.
    pub async fn commit_collection_in(
        &self,
        tx: &DatabaseTransaction,
        received: ReceivedArtifact,
    ) -> Result<ArtifactRef> {
        lock_dispatch(tx, &received.actor.dispatch).await?;
        let attached = self
            .validate_collection(tx, &received.admin, &received.actor, &received.artifact)
            .await?;
        if attached.generation != received.actor.generation
            || attached.current_run_id.as_deref() != Some(&received.actor.run)
        {
            return Err(ExchangeError::Forbidden);
        }
        let artifact = &received.artifact;
        if let Some(row)=tx.query_one(Statement::from_sql_and_values(DbBackend::Sqlite,"SELECT dispatch_id,artifact_json FROM hall_exchange_collection WHERE object_id=? OR (dispatch_id=? AND attempt_id=? AND slot_id=?)",vec![artifact.object.object_id.clone().into(),artifact.dispatch_id.clone().into(),artifact.attempt_id.clone().into(),artifact.slot_id.clone().into()])).await? {
            if row.try_get::<String>("","dispatch_id")?!=artifact.dispatch_id {return Err(ExchangeError::Forbidden);}
            let original:ArtifactRef=serde_json::from_str(&row.try_get::<String>("","artifact_json")?).map_err(|_|ExchangeError::Storage)?;
            if original!=*artifact {return Err(ExchangeError::Conflict);}return Ok(original);
        }
        let bytes=tx.query_one(Statement::from_sql_and_values(DbBackend::Sqlite,"SELECT COALESCE(SUM(b.size),0) AS bytes FROM hall_exchange_collection c JOIN hall_exchange_blob b ON b.blob_id=c.blob_id WHERE c.dispatch_id=? AND c.attempt_id=?",vec![artifact.dispatch_id.clone().into(),artifact.attempt_id.clone().into()])).await?.ok_or(ExchangeError::Storage)?.try_get::<i64>("","bytes")? as u64;
        let input_bytes: u64 = attached_manifest(tx, &attached)
            .await?
            .inputs
            .iter()
            .map(|i| i.size)
            .sum();
        if bytes + input_bytes + artifact.object.size > MAX_TOTAL_BYTES {
            return Err(ExchangeError::Quota);
        }
        let blob = self
            .publish_temporary(received.temporary, artifact.object.size)
            .await?;
        persist_blob(tx, &blob, artifact.object.size, &artifact.object.sha256).await?;
        tx.execute(Statement::from_sql_and_values(DbBackend::Sqlite,"INSERT INTO hall_exchange_collection(object_id,dispatch_id,attempt_id,run_id,slot_id,blob_id,artifact_json,collected_at) VALUES(?,?,?,?,?,?,?,?)",vec![artifact.object.object_id.clone().into(),artifact.dispatch_id.clone().into(),artifact.attempt_id.clone().into(),artifact.run_id.clone().into(),artifact.slot_id.clone().into(),blob.into(),serde_json::to_string(artifact).map_err(|_|ExchangeError::Storage)?.into(),Utc::now().into()])).await?;
        Ok(received.artifact)
    }
    pub async fn collected_artifacts(
        &self,
        admin: &Administrator,
        dispatch: &str,
    ) -> Result<Vec<ArtifactRef>> {
        let row = self
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT submission_id FROM hall_exchange_attachment WHERE dispatch_id=?",
                vec![dispatch.to_owned().into()],
            ))
            .await?
            .ok_or(ExchangeError::Forbidden)?;
        owns(
            &self.db,
            admin,
            &row.try_get::<String>("", "submission_id")?,
        )
        .await?;
        let rows=self.db.query_all(Statement::from_sql_and_values(DbBackend::Sqlite,"SELECT artifact_json FROM hall_exchange_collection WHERE dispatch_id=? ORDER BY collected_at,object_id",vec![dispatch.to_owned().into()])).await?;
        rows.into_iter()
            .map(|r| {
                serde_json::from_str(&r.try_get::<String>("", "artifact_json")?)
                    .map_err(|_| ExchangeError::Storage)
            })
            .collect()
    }
    pub async fn open_collected(&self, admin: &Administrator, object: &str) -> Result<File> {
        let row=self.db.query_one(Statement::from_sql_and_values(DbBackend::Sqlite,"SELECT c.blob_id,c.artifact_json FROM hall_exchange_collection c JOIN hall_exchange_attachment a ON a.dispatch_id=c.dispatch_id JOIN hall_exchange_submission s ON s.submission_id=a.submission_id WHERE c.object_id=? AND s.owner_id=?",vec![object.to_owned().into(),admin.owner.clone().into()])).await?.ok_or(ExchangeError::Forbidden)?;
        let artifact: ArtifactRef =
            serde_json::from_str(&row.try_get::<String>("", "artifact_json")?)
                .map_err(|_| ExchangeError::Storage)?;
        let blob: String = row.try_get("", "blob_id")?;
        let files = self.files.clone();
        tokio::task::spawn_blocking(move || {
            files.blob(&blob, artifact.object.size, &artifact.object.sha256)
        })
        .await
        .map_err(|_| ExchangeError::Storage)?
    }
}

/// Host-trusted view of a dispatch's frozen attachment. Built from the ledger,
/// never from a request, so it can mint owners for HQ delivery and the runtime.
pub struct DispatchBinding {
    pub owner: DispatchOwner,
    pub submission: String,
    pub manifest: ExchangeManifest,
}
impl Exchange {
    pub async fn dispatch_binding(&self, dispatch: &str) -> Result<Option<DispatchBinding>> {
        uuid(dispatch)?;
        let Some(row) = Attachment::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT * FROM hall_exchange_attachment WHERE dispatch_id=?",
            vec![dispatch.to_owned().into()],
        ))
        .one(&self.db)
        .await?
        else {
            return Ok(None);
        };
        let owner = DispatchOwner::authenticated(&row.issuer_id, dispatch, &row.branch_id)?;
        let manifest = attached_manifest(&self.db, &row).await?;
        Ok(Some(DispatchBinding {
            owner,
            submission: row.submission_id,
            manifest,
        }))
    }
    /// True only when every manifest input is fully verified on disk here.
    pub async fn inputs_ready(&self, owner: &DispatchOwner) -> Result<bool> {
        let attached = attachment(&self.db, owner).await?;
        if attached.revoked {
            return Ok(false);
        }
        let manifest = attached_manifest(&self.db, &attached).await?;
        for object in &manifest.inputs {
            let row = input(
                &self.db,
                &attached.submission_id,
                InputKey::Object(&object.object_id),
            )
            .await?;
            if !row.is_some_and(|r| r.confirmed && r.blob_id.is_some()) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
