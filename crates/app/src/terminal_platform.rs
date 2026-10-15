//! Small OS boundary for PTY ownership. The Windows job is established before
//! accepting input; on Unix the session leader is not reaped until its groups
//! have been stopped, preventing PID reuse during cleanup.
use portable_pty::{Child, MasterPty};
use std::io;

/// PowerShell exposes verbatim Win32 paths as provider-qualified locations;
/// tools such as Node then fail resolving relative paths. Use an ordinary
/// spelling only when it resolves back to the exact registered directory.
pub(crate) fn shell_directory(root: &std::path::Path) -> std::path::PathBuf {
    #[cfg(windows)]
    if let Some(path) = root.to_str() {
        let ordinary = if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
            Some(std::path::PathBuf::from(format!(r"\\{path}")))
        } else {
            path.strip_prefix(r"\\?\").map(std::path::PathBuf::from)
        };
        if let Some(ordinary) = ordinary
            && ordinary
                .canonicalize()
                .is_ok_and(|resolved| resolved == root)
        {
            return ordinary;
        }
    }
    root.to_owned()
}

#[cfg(windows)]
pub(crate) struct Ownership(std::os::windows::io::OwnedHandle);

#[cfg(windows)]
impl Ownership {
    pub(crate) fn attach(child: &dyn Child, _master: &dyn MasterPty) -> io::Result<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::JobObjects::*;
        // SAFETY: default security and an unnamed, newly owned job.
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateJobObjectW returned this new, exclusively owned handle.
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: correctly typed, live limits and job handle.
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let handle = child
            .as_raw_handle()
            .ok_or_else(|| io::Error::other("terminal process handle is unavailable"))?;
        // SAFETY: the child handle is borrowed from the live process we spawned.
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), handle) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(job))
    }
    pub(crate) fn has_exited(&self, child: &mut dyn Child) -> io::Result<bool> {
        Ok(child.try_wait()?.is_some())
    }
    pub(crate) fn terminate(&self, _master: &dyn MasterPty) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        // SAFETY: this job owns only our terminal shell and its descendants.
        if unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0.as_raw_handle(), 1)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(unix)]
pub(crate) struct Ownership(libc::pid_t);

#[cfg(unix)]
impl Ownership {
    pub(crate) fn attach(child: &dyn Child, master: &dyn MasterPty) -> io::Result<Self> {
        let pid = child
            .process_id()
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|pid| *pid > 0)
            .ok_or_else(|| io::Error::other("terminal process id is unavailable"))?;
        let fd = master
            .as_raw_fd()
            .ok_or_else(|| io::Error::other("terminal descriptor is unavailable"))?;
        // SAFETY: fd is borrowed from the live PTY. Clones share these flags,
        // allowing reader/writer threads to check shutdown without blocking.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(pid))
    }
    pub(crate) fn has_exited(&self, _child: &mut dyn Child) -> io::Result<bool> {
        // SAFETY: initialized siginfo storage, and the still-owned direct child.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::waitid(
                libc::P_PID,
                self.0 as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: waitid initialized info; a zero pid means no child state change.
        Ok(unsafe { info.si_pid() } != 0)
    }
    pub(crate) fn terminate(&self, master: &dyn MasterPty) -> io::Result<()> {
        let foreground = master
            .process_group_leader()
            .filter(|pid| *pid > 0 && *pid != self.0);
        for group in [foreground, Some(self.0)].into_iter().flatten() {
            // SAFETY: PTY groups belong to the child session; its unreaped
            // leader reserves the id until cleanup completes.
            if unsafe { libc::kill(-group, libc::SIGKILL) } < 0 {
                let error = io::Error::last_os_error();
                // macOS reports EPERM when an unreaped group contains only
                // zombies. Confirm absence without sending another signal.
                let absent = error.raw_os_error() == Some(libc::ESRCH)
                    || (error.raw_os_error() == Some(libc::EPERM)
                        && unsafe { libc::kill(-group, 0) } < 0
                        && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH));
                if !absent {
                    return Err(error);
                }
            }
        }
        Ok(())
    }
    pub(crate) fn hang_up(&self) -> io::Result<()> {
        // SAFETY: the direct child has not been reaped, so its pid is still
        // reserved. Bash propagates SIGHUP to its foreground/background jobs.
        if unsafe { libc::kill(self.0, libc::SIGHUP) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
