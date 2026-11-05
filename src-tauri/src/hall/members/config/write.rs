//! Fixed overlay destination. Cooperative file lock + optimistic content
//! checks + same-directory atomic replacement, not a filesystem transaction.
use super::super::HallMember;
use super::{combine, read, MemberConfigPreview};
use crate::app_error::AppCommandError;
use std::path::Path;

#[cfg(unix)]
#[path = "write_unix.rs"]
mod platform;
#[cfg(not(unix))]
#[path = "write_other.rs"]
mod platform;

pub(super) fn conflict() -> AppCommandError {
    AppCommandError::already_exists(
        "Configuration changed or another save is active; read it again",
    )
}
pub(super) fn failed() -> AppCommandError {
    AppCommandError::io_error("Save could not be confirmed; read the configuration again")
}

pub(super) fn replace(
    member: &HallMember,
    version: &str,
    bytes: &[u8],
) -> Result<MemberConfigPreview, AppCommandError> {
    replace_checked(member, version, bytes, || {})
}

fn replace_checked(
    member: &HallMember,
    version: &str,
    bytes: &[u8],
    before_commit: impl FnOnce(),
) -> Result<MemberConfigPreview, AppCommandError> {
    let root = read::Root::open(Path::new(&member.cwd)).map_err(|_| failed())?;
    let current = combine(member, Some(read::read_from(&root)));
    if !current.valid || current.file_version.as_deref() != Some(version) {
        return Err(conflict());
    }
    let mut writer = platform::Writer::open(&root, Path::new(&member.cwd))?;
    writer.stage(bytes)?;
    before_commit();
    writer.validate(&root, Path::new(&member.cwd))?;
    let mut files = read::read_from(&root);
    let current = combine(member, Some(files.clone()));
    if !current.valid || current.file_version.as_deref() != Some(version) {
        return Err(conflict());
    }
    files[2] = read::ReadFile {
        path: read::PATHS[2],
        data: Some(bytes.to_vec()),
        error: None,
    };
    let expected = combine(member, Some(files));
    if !expected.valid {
        return Err(failed());
    }
    writer.commit()?;
    let result = combine(member, Some(read::read_from(&root)));
    if !result.valid || result.file_version != expected.file_version {
        return Err(failed());
    }
    Ok(result)
}

#[cfg(test)]
#[path = "write_tests.rs"]
mod tests;
