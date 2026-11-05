//! Native probes run through the same ACP factory and domain as real launches.
use super::probe::{failed, NativeCertificate, Owned, ProbeReport};
use super::{platform::Group, Coverage, Domain, ExecutionOwner};
use crate::app_error::AppCommandError;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
pub(super) async fn verify(path: &Path, nonce: &str) -> Result<NativeCertificate, AppCommandError> {
    use agent_client_protocol::schema::v1::{McpServer, McpServerStdio};
    let group = Arc::new(Group::open(path, nonce).map_err(|_| failed())?);
    let owner = ExecutionOwner {
        member: nonce.into(),
        task: nonce.into(),
        attempt: nonce.into(),
        snapshot: nonce.into(),
        claim: uuid::Uuid::new_v4().to_string(),
        epoch: uuid::Uuid::new_v4().to_string(),
    };
    let domain = Domain::new(owner, Coverage::Managed(group.clone()));
    let mut owned = Owned {
        domain: domain.clone(),
        children: Vec::new(),
    };
    for _ in 0..2 {
        #[cfg(windows)]
        let mut command = {
            let mut c = tokio::process::Command::new("powershell.exe");
            c.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                include_str!("windows-control.ps1"),
            ]);
            c.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped());
            c
        };
        #[cfg(target_os = "macos")]
        let mut command = {
            let mut c = tokio::process::Command::new("/bin/sleep");
            c.arg("120");
            c
        };
        owned
            .children
            .push(command.kill_on_drop(true).spawn().map_err(|_| failed())?);
    }
    #[cfg(windows)]
    for child in &mut owned.children {
        let mut line = String::new();
        tokio::time::timeout(
            Duration::from_secs(40),
            BufReader::new(child.stdout.take().ok_or_else(failed)?).read_line(&mut line),
        )
        .await
        .map_err(|_| failed())?
        .map_err(|_| failed())?;
        if line.trim().parse::<u32>().ok() != child.id()
            || child.try_wait().map_err(|_| failed())?.is_some()
        {
            return Err(failed());
        }
    }
    let controls = owned
        .children
        .iter()
        .map(|c| c.id().and_then(super::process::birth).ok_or_else(failed))
        .collect::<Result<Vec<_>, _>>()?;
    #[cfg(windows)]
    let server = McpServerStdio::new("onehall-native-probe", "powershell.exe").args(vec![
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-Command".into(),
        include_str!("windows-probe.ps1").into(),
    ]);
    #[cfg(target_os = "macos")]
    let server = McpServerStdio::new("onehall-native-probe", "/bin/sh").args(vec![
        "-c".into(),
        "sleep 120 & middle=$!; echo $$ $middle; read line; sleep 120 & echo $!; wait".into(),
    ]);
    let agent = crate::acp::agent_process::AcpAgent::new(McpServer::Stdio(server))
        .with_process_domain(Some(domain.clone()));
    let (mut input, output, _stderr, child) = agent.spawn_process().map_err(|_| failed())?;
    owned.children.push(child);
    let mut reader = BufReader::new(output);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(40), reader.read_line(&mut line))
        .await
        .map_err(|_| failed())?
        .map_err(|_| failed())?;
    let ids = line
        .split_whitespace()
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| failed())?;
    let expected = if cfg!(windows) { 3 } else { 2 };
    if ids.len() != expected || owned.children[2].id() != ids.first().copied() {
        return Err(failed());
    }
    let mut births = ids
        .iter()
        .map(|pid| super::process::birth(*pid).ok_or_else(failed))
        .collect::<Result<Vec<_>, _>>()?;
    input.write_all(b"late\n").await.map_err(|_| failed())?;
    line.clear();
    tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
        .await
        .map_err(|_| failed())?
        .map_err(|_| failed())?;
    let late = line.trim().parse().map_err(|_| failed())?;
    births.push(super::process::birth(late).ok_or_else(failed)?);
    domain.cancel().map_err(|_| failed())?;
    tokio::time::timeout(Duration::from_secs(10), owned.children[2].wait())
        .await
        .map_err(|_| failed())?
        .map_err(|_| failed())?;
    domain.root_reaped();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let known_gone = births.iter().all(|b| super::process::gone(*b));
        let report = domain.observation();
        let mechanism_passed = if cfg!(windows) {
            report.complete && known_gone
        } else {
            known_gone && report.root_reaped && !report.complete
        };
        if mechanism_passed {
            break;
        }
        if Instant::now() >= deadline {
            return Err(failed());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let unrelated_alive = controls
        .iter()
        .zip(owned.children.iter_mut())
        .all(|(birth, child)| {
            super::process::birth(birth.pid) == Some(*birth)
                && child.try_wait().is_ok_and(|s| s.is_none())
        });
    if !unrelated_alive {
        return Err(failed());
    }
    let observation = domain.observation();
    Ok(NativeCertificate {
        issued: Instant::now(),
        report: ProbeReport {
            probe_id: nonce.into(),
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            coverage: observation.coverage,
            complete: observation.complete,
            mechanism_verified: true,
            unrelated_alive,
            observed_processes: births.len(),
        },
    })
}
