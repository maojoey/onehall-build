//! Owned Job Object; no breakaway, kill-on-close. User code is suspended until
//! assignment succeeds. Every failure terminates and waits for our exact child.
use std::{
    io,
    mem::{size_of, zeroed},
};
use tokio::process::{Child, Command};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
            TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Threading::{
            GetExitCodeProcess, GetProcessIdOfThread, OpenThread, ResumeThread, TerminateProcess,
            WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED, INFINITE,
            THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
        },
    },
};
struct Handle(HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
pub(crate) struct NativeGroup {
    job: Handle,
}
impl NativeGroup {
    pub(crate) fn new() -> io::Result<Self> {
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let group = Self { job: Handle(job) };
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(group)
    }
    pub(crate) fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        command
            .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        let result = self.assign_and_resume(&child);
        if let Err(error) = result {
            if let Some(handle) = child.raw_handle() {
                // A still-suspended owned process must not be abandoned. No PID lookup.
                unsafe {
                    TerminateProcess(handle.cast(), 1);
                    WaitForSingleObject(handle.cast(), INFINITE);
                }
            }
            let _ = child.try_wait();
            return Err(error);
        }
        Ok(child)
    }
    fn assign_and_resume(&self, child: &Child) -> io::Result<()> {
        let handle = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("Missing owned child handle"))?;
        if unsafe { AssignProcessToJobObject(self.job.0, handle.cast()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("Missing owned child identity"))?;
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = Handle(snapshot);
        let mut entry: THREADENTRY32 = unsafe { zeroed() };
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut next = unsafe { Thread32First(snapshot.0, &mut entry) };
        let mut thread = None;
        while next != 0 {
            if entry.th32OwnerProcessID == pid {
                if thread.is_some() {
                    return Err(io::Error::other("Unexpected threads before native launch"));
                }
                let raw = unsafe {
                    OpenThread(
                        THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
                        0,
                        entry.th32ThreadID,
                    )
                };
                if raw.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let owned = Handle(raw);
                if unsafe { GetProcessIdOfThread(raw) } != pid {
                    return Err(io::Error::other("Child thread ownership changed"));
                }
                thread = Some(owned);
            }
            next = unsafe { Thread32Next(snapshot.0, &mut entry) };
        }
        let thread =
            thread.ok_or_else(|| io::Error::other("Suspended child thread unavailable"))?;
        let mut exit = 0;
        if unsafe { GetExitCodeProcess(handle.cast(), &mut exit) } == 0 || exit != 259 {
            return Err(io::Error::other("Child exited before assignment"));
        }
        if unsafe { ResumeThread(thread.0) } != 1 {
            return Err(io::Error::other(
                "Native child did not resume from creation suspension",
            ));
        }
        Ok(())
    }
    pub(crate) fn cancel(&self) -> io::Result<()> {
        if unsafe { TerminateJobObject(self.job.0, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub(crate) fn populated(&self) -> io::Result<bool> {
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
        if unsafe {
            QueryInformationJobObject(
                self.job.0,
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(info.ActiveProcesses != 0)
    }
}
#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
