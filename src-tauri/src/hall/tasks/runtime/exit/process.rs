//! Read identity, never signal by a bare PID. No command line/environment reads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Birth {
    pub pid: u32,
    pub start: u64,
    pub fraction: u64,
}
#[cfg(target_os = "linux")]
pub(crate) fn birth(pid: u32) -> Option<Birth> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let tail = stat.rsplit_once(") ")?.1;
    let fields: Vec<_> = tail.split_whitespace().collect();
    Some(Birth {
        pid,
        start: fields.get(19)?.parse().ok()?,
        fraction: 0,
    })
}
#[cfg(target_os = "macos")]
pub(crate) fn birth(pid: u32) -> Option<Birth> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // Kernel-owned start timestamp; no user/model process-name assertion.
    let read = unsafe {
        libc::proc_pidinfo(
            pid.try_into().ok()?,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size.try_into().ok()?,
        )
    };
    if read != size as i32 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    Some(Birth {
        pid: info.pbi_pid,
        start: info.pbi_start_tvsec,
        fraction: info.pbi_start_tvusec,
    })
}
#[cfg(windows)]
pub(crate) fn birth(pid: u32) -> Option<Birth> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME},
        System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let mut created: FILETIME = unsafe { std::mem::zeroed() };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    let ok = unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) };
    unsafe {
        CloseHandle(handle);
    }
    (ok != 0).then_some(Birth {
        pid,
        start: (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
        fraction: 0,
    })
}
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(crate) fn birth(_: u32) -> Option<Birth> {
    None
}

/// Identity can outlive execution on Windows while another handle retains the
/// process object. Require a signaled owned query handle, not PID disappearance.
#[cfg(windows)]
pub(crate) fn gone(old: Birth) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, FILETIME, WAIT_OBJECT_0},
        System::Threading::{
            GetProcessTimes, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };
    // Standard SYNCHRONIZE access is required for WaitForSingleObject.
    const SYNCHRONIZE: u32 = 0x0010_0000;
    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, old.pid) };
    if handle.is_null() {
        // Nonexistent PID, unlike access denied or any unknown observer error.
        return unsafe { GetLastError() } == ERROR_INVALID_PARAMETER;
    }
    let mut created: FILETIME = unsafe { std::mem::zeroed() };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    let read =
        unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) };
    let start = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    let gone = read != 0
        && (start != old.start || unsafe { WaitForSingleObject(handle, 0) } == WAIT_OBJECT_0);
    unsafe {
        CloseHandle(handle);
    }
    gone
}
#[cfg(target_os = "macos")]
pub(crate) fn gone(old: Birth) -> bool {
    birth(old.pid) != Some(old)
}
