//! Real, no-model host probe. Opens ONLY a pre-provisioned empty per-probe
//! cgroup v2; no mounts, permissions, controllers or directories are changed.
//! macOS polling cannot certify detached descendants and is never promoted.
use super::Domain;
#[cfg(not(any(windows, target_os = "macos")))]
use super::{cgroup::Cgroup, Coverage, ExecutionOwner};
use crate::app_error::AppCommandError;
use serde::Serialize;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::process::Child;
pub(crate) struct NativeCertificate {
    pub(super) issued: Instant,
    pub(super) report: ProbeReport,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReport {
    pub probe_id: String,
    pub platform: String,
    pub coverage: String,
    pub complete: bool,
    pub mechanism_verified: bool,
    pub unrelated_alive: bool,
    pub observed_processes: usize,
}
impl NativeCertificate {
    pub(crate) fn valid(&self) -> bool {
        self.issued.elapsed() < Duration::from_secs(900)
            && self.report.mechanism_verified
            && self.report.unrelated_alive
            && self.report.coverage == super::platform::coverage()
            && (self.report.complete || cfg!(target_os = "macos"))
    }
    pub(crate) fn report(&self) -> ProbeReport {
        self.report.clone()
    }
}
pub(super) struct Owned {
    pub(super) domain: Arc<Domain>,
    pub(super) children: Vec<Child>,
}
impl Drop for Owned {
    fn drop(&mut self) {
        let _ = self.domain.cancel();
        for c in &mut self.children {
            let _ = c.start_kill();
        }
        let children = std::mem::take(&mut self.children);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                for mut c in children {
                    let _ = c.wait().await;
                }
            });
        }
    }
}
pub(super) fn failed() -> AppCommandError {
    AppCommandError::permission_denied("Native B10 could not be verified; no admission was issued")
}
pub(crate) async fn verify(path: &Path, nonce: &str) -> Result<NativeCertificate, AppCommandError> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        super::probe_native::verify(path, nonce).await
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        verify_linux(path, nonce).await
    }
}
#[cfg(not(any(windows, target_os = "macos")))]
async fn verify_linux(path: &Path, nonce: &str) -> Result<NativeCertificate, AppCommandError> {
    use agent_client_protocol::schema::v1::{McpServer, McpServerStdio};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    if !cfg!(target_os = "linux") {
        return Err(AppCommandError::dependency_missing(
            "Complete native descendant coverage is not available on this \
     platform",
        ));
    }
    let group = Arc::new(Cgroup::open(path, nonce).map_err(|_| failed())?);
    let owner = ExecutionOwner {
        member: nonce.into(),
        task: nonce.into(),
        attempt: nonce.into(),
        snapshot: nonce.into(),
        claim: uuid::Uuid::new_v4().to_string(),
        epoch: uuid::Uuid::new_v4().to_string(),
    };
    let domain = Domain::new(owner, Coverage::Linux(group));
    let mut owned = Owned {
        domain: domain.clone(),
        children: Vec::new(),
    };
    // Two explicit owned controls outside the target domain. No process scan or
    // kill-by-name; every signal below uses a Child owned by this invocation.
    for _ in 0..2 {
        owned.children.push(
            tokio::process::Command::new("/bin/sleep")
                .arg("120")
                .spawn()
                .map_err(|_| failed())?,
        );
    }
    let controls = owned
        .children
        .iter()
        .map(|c| c.id().and_then(super::process::birth).ok_or_else(failed))
        .collect::<Result<Vec<_>, _>>()?;
    let mid = "import os,subprocess,time,\
     json\np=subprocess.Popen(['/bin/sleep','120'],\
     start_new_session=True)\nprint(json.dumps([os.getpid(),p.pid]),\
     flush=True)\ntime.sleep(120)\n";
    let code = format!(
        "import os,sys,subprocess,json,\
     time\np=subprocess.Popen([sys.executable,'-u','-c',{}],\
     stdout=subprocess.PIPE,\
     text=True)\nids=json.loads(p.stdout.readline());print(json.dumps([os.getpid()]+ids),\
     flush=True)\nsys.stdin.readline()\nq=subprocess.Popen(['/bin/sleep',\
     '120'],start_new_session=True)\nprint(q.pid,\
     flush=True)\ntime.sleep(120)\n",
        serde_json::to_string(mid).map_err(|_| failed())?
    );
    let agent =
        crate::acp::agent_process::AcpAgent::new(McpServer::Stdio(
            McpServerStdio::new("onehall-native-b10", std::path::PathBuf::from("python3"))
                .args(vec!["-u".into(), "-c".into(), code]),
        ))
        .with_process_domain(Some(domain.clone()));
    let (mut input, output, _stderr, child) = agent.spawn_process().map_err(|_| failed())?;
    owned.children.push(child);
    let mut reader = BufReader::new(output);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(8), reader.read_line(&mut line))
        .await
        .map_err(|_| failed())?
        .map_err(|_| failed())?;
    let ids: Vec<u32> = serde_json::from_str(&line).map_err(|_| failed())?;
    if ids.len() != 3 || ids.iter().any(|pid| super::process::birth(*pid).is_none()) {
        return Err(failed());
    }
    input.write_all(b"late\n").await.map_err(|_| failed())?;
    line.clear();
    tokio::time::timeout(Duration::from_secs(8), reader.read_line(&mut line))
        .await
        .map_err(|_| failed())?
        .map_err(|_| failed())?;
    let late: u32 = line.trim().parse().map_err(|_| failed())?;
    if super::process::birth(late).is_none() {
        return Err(failed());
    }
    domain.cancel().map_err(|_| failed())?;
    tokio::time::timeout(Duration::from_secs(8), owned.children[2].wait())
        .await
        .map_err(|_| failed())?
        .map_err(|_| failed())?;
    domain.root_reaped();
    let mut observation = domain.observation();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !observation.complete && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
        observation = domain.observation();
    }
    let unrelated_alive = controls
        .iter()
        .zip(owned.children.iter_mut())
        .all(|(birth, child)| {
            super::process::birth(birth.pid) == Some(*birth)
                && child.try_wait().is_ok_and(|s| s.is_none())
        });
    if !observation.complete || !unrelated_alive {
        return Err(failed());
    }
    // Certificate is private and boot-local. Exported reports are not accepted
    // back as evidence by any API, even if every JSON field is copied.
    Ok(NativeCertificate {
        issued: Instant::now(),
        report: ProbeReport {
            probe_id: nonce.into(),
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            coverage: observation.coverage,
            complete: true,
            mechanism_verified: true,
            unrelated_alive,
            observed_processes: 4,
        },
    })
}
