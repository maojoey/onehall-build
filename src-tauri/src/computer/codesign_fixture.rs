//! Test-only, freshly compiled images. Never signs the Cargo test process or
//! an installed binary. The owning process guard reaps its child before the
//! private directory (and its executable) is removed, including on panic.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub(super) const IDENTIFIER: &str = "onehall.codesign-test-fixture";
pub(super) const REQUIREMENT: &str = "identifier \"onehall.codesign-test-fixture\"";

const SOURCE: &str = r#"
#include <errno.h>
#include <stdio.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    FILE *ready = fopen(argv[1], "w");
    if (!ready) return 3;
    if (fputs("ready\n", ready) < 0) return 4;
    if (fclose(ready) != 0) return 5;
    char byte;
    ssize_t count;
    do { count = read(STDIN_FILENO, &byte, 1); }
    while (count < 0 && errno == EINTR);
    return count == 0 ? 0 : 6;
}
"#;

fn checked(command: &mut Command) {
    let output = command.output().expect("run fixture tool");
    assert!(
        output.status.success(),
        "fixture tool {command:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(super) struct Image {
    pub(super) binary: PathBuf,
    ready: PathBuf,
    _dir: tempfile::TempDir,
}

impl Image {
    pub(super) fn new(signed: bool) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("onehall-codesign-fixture-")
            .tempdir()
            .expect("private fixture directory");
        let source = dir.path().join("fixture.c");
        let binary = dir.path().join("fixture");
        std::fs::write(&source, SOURCE).expect("write fresh fixture source");
        checked(
            Command::new("/usr/bin/xcrun")
                .args(["clang", "-Wall", "-Wextra", "-Werror"])
                .arg(&source)
                .arg("-o")
                .arg(&binary),
        );
        // arm64 linkers may already sign their output; x86_64 need not.
        // Normalize only this newly built fixture, without a key or certificate.
        checked(
            Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-", "--timestamp=none", "--identifier"])
                .arg(IDENTIFIER)
                .arg(&binary),
        );
        if !signed {
            checked(
                Command::new("/usr/bin/codesign")
                    .arg("--remove-signature")
                    .arg(&binary),
            );
        }
        Self {
            binary,
            ready: dir.path().join("ready"),
            _dir: dir,
        }
    }

    pub(super) fn spawn(self) -> Process {
        let child = Command::new(&self.binary)
            .arg(&self.ready)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn only the freshly compiled fixture");
        let mut process = Process { child, _image: self };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            assert!(
                process.child.try_wait().expect("fixture status").is_none(),
                "fixture exited before its ready handshake"
            );
            if std::fs::read_to_string(&process._image.ready).is_ok_and(|s| s == "ready\n") {
                break;
            }
            assert!(Instant::now() < deadline, "fixture ready handshake timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
        process
    }
}

pub(super) struct Process {
    child: Child,
    _image: Image,
}

impl Process {
    pub(super) fn id(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Closing our pipe lets the fixture exit; kill only this owned child if
        // it has not exited yet, then reap it before TempDir removes the image.
        drop(self.child.stdin.take());
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}
