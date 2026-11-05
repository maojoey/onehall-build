use super::NativeGroup;
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};
#[cfg(target_os = "macos")]
#[tokio::test]
async fn independent_group_cancellation_stops_owned_children_and_preserves_control() {
    let group = NativeGroup::new().unwrap();
    let mut control = Command::new("/bin/sleep")
        .arg("30")
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "sleep 30 & echo $!; wait"])
        .stdout(Stdio::piped());
    let mut child = group.spawn(&mut cmd).unwrap();
    let pid = child.id().unwrap();
    let mut line = String::new();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let descendant: i32 = line.trim().parse().unwrap();
    assert_eq!(unsafe { libc::getpgid(pid as i32) }, pid as i32);
    assert_eq!(unsafe { libc::getpgid(descendant) }, pid as i32);
    assert_ne!(unsafe { libc::getpgrp() }, pid as i32);
    group.cancel().unwrap();
    tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
    let mut tail = String::new();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut tail))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    assert!(control.try_wait().unwrap().is_none());
    assert!(
        group.populated().is_err(),
        "Mac must never certify all descendants"
    );
    control.kill().await.unwrap();
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn stale_birth_does_not_authorize_signalling_a_live_unrelated_group() {
    let group = NativeGroup::new().unwrap();
    let mut cmd = Command::new("/bin/sleep");
    cmd.arg("30").process_group(0).kill_on_drop(true);
    let mut child = cmd.spawn().unwrap();
    let mut wrong = super::birth(child.id().unwrap()).unwrap();
    wrong.start += 1;
    group.leaders.lock().unwrap().push(wrong);
    group.cancel().unwrap();
    assert!(child.try_wait().unwrap().is_none());
    child.kill().await.unwrap();
}
#[cfg(windows)]
#[tokio::test]
async fn headless_job_tree_is_assigned_before_user_code_and_rejects_breakaway() {
    let group = NativeGroup::new().unwrap();
    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        include_str!("windows-probe.ps1"),
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped());
    let mut child = group.spawn(&mut cmd).unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(30), reader.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let mut ids = line
        .split_whitespace()
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(ids.len(), 3, "Probe must create three generations");
    assert_eq!(ids[0], child.id().unwrap());
    let in_exact_job = |pid| {
        use windows_sys::Win32::System::{
            JobObjects::IsProcessInJob,
            Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        };
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        assert!(!raw.is_null());
        let handle = super::Handle(raw);
        let mut contained = 0;
        assert_ne!(
            unsafe { IsProcessInJob(handle.0, group.job.0, &mut contained) },
            0
        );
        assert_ne!(
            contained, 0,
            "Fixture descendant escaped the exact owned Job"
        );
    };
    for pid in &ids {
        in_exact_job(*pid);
    }
    use tokio::io::AsyncWriteExt;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"late\n")
        .await
        .unwrap();
    line.clear();
    tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let late = line.trim().parse::<u32>().unwrap();
    in_exact_job(late);
    ids.push(late);
    let births = ids
        .iter()
        .map(|pid| super::super::process::birth(*pid).unwrap())
        .collect::<Vec<_>>();
    assert!(births.iter().all(|old| !super::super::process::gone(*old)));
    assert!(child.try_wait().unwrap().is_none());
    assert!(group.populated().unwrap());
    group.cancel().unwrap();
    tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
    for _ in 0..100 {
        if !group.populated().unwrap() && births.iter().all(|b| super::super::process::gone(*b)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("Job still populated after cancellation");
}

#[cfg(windows)]
#[tokio::test]
async fn assignment_failure_never_runs_user_code() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("must-not-exist");
    // An invalid Job provokes an OS assignment failure after creation suspended.
    let group = NativeGroup {
        job: super::Handle(std::ptr::null_mut()),
    };
    let mut cmd = Command::new("cmd.exe");
    cmd.args(["/c", &format!("echo escaped>\"{}\"", marker.display())]);
    assert!(group.spawn(&mut cmd).is_err());
    assert!(!marker.exists());
}
#[cfg(windows)]
#[tokio::test]
async fn closing_job_kills_owned_root_but_keeps_control_alive() {
    let mut control = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            include_str!("windows-control.ps1"),
        ])
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(40),
        BufReader::new(control.stdout.take().unwrap()).read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(line.trim().parse::<u32>().unwrap(), control.id().unwrap());
    assert!(control.try_wait().unwrap().is_none());
    let group = NativeGroup::new().unwrap();
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-Command", "Start-Sleep 120"]);
    let mut child = group.spawn(&mut cmd).unwrap();
    assert!(group.populated().unwrap());
    drop(group);
    tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(control.try_wait().unwrap().is_none());
    control.kill().await.unwrap();
}
#[cfg(windows)]
#[tokio::test]
async fn ordinary_exit_leaves_the_job_empty() {
    let group = NativeGroup::new().unwrap();
    let mut cmd = Command::new("cmd.exe");
    cmd.args(["/c", "exit", "0"]);
    let mut child = group.spawn(&mut cmd).unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    // A waited process is exited; Job accounting is updated asynchronously.
    // Keep the strong kernel-domain oracle and bound the settling interval.
    for _ in 0..100 {
        if !group.populated().unwrap() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("Job accounting remained populated after ordinary exit");
}

#[cfg(windows)]
#[tokio::test]
async fn retained_process_handle_is_exited_evidence_not_a_live_pid() {
    use std::os::windows::io::BorrowedHandle;
    let group = NativeGroup::new().unwrap();
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-Command", "Start-Sleep 120"]);
    let mut child = group.spawn(&mut cmd).unwrap();
    let identity = super::super::process::birth(child.id().unwrap()).unwrap();
    let retained = unsafe { BorrowedHandle::borrow_raw(child.raw_handle().unwrap()) }
        .try_clone_to_owned()
        .unwrap();
    assert!(!super::super::process::gone(identity));
    child.kill().await.unwrap();
    assert!(super::super::process::birth(identity.pid) == Some(identity));
    assert!(super::super::process::gone(identity));
    drop(retained);
}
