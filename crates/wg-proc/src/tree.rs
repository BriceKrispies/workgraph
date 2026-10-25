//! Whole-tree termination: a Job Object on Windows, a process group on POSIX.

use std::io;
use std::process::Command;
use std::time::{Duration, Instant};

/// How long a terminated child may take to be reaped before it is killed directly.
const REAP_TIMEOUT: Duration = Duration::from_secs(10);

/// Make the child the leader of a new process group (POSIX) or process group (Windows), so a
/// console interrupt aimed at us is not delivered to it and the tree can be addressed as a unit.
pub(crate) fn configure(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_PROCESS_GROUP
        command.creation_flags(0x0000_0200);
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = command;
    }
}

/// Owns whatever is needed to terminate a started child and its descendants.
pub(crate) struct TreeGuard {
    #[cfg(windows)]
    job: Option<windows_job::Job>,
}

impl TreeGuard {
    /// Take control of a freshly spawned child. On Windows the child is put in a Job Object that
    /// kills everything in it when the guard is dropped. There is a tiny window between spawn and
    /// assignment in which the child could already have created a grandchild; it is not covered.
    pub(crate) fn attach(child: &std::process::Child) -> Self {
        #[cfg(windows)]
        {
            Self {
                job: windows_job::Job::create_and_assign(child),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = child;
            Self {}
        }
    }

    /// Terminate the child and its descendants and reap the child.
    pub(crate) fn kill_tree(&self, child: &mut std::process::Child) -> io::Result<()> {
        if child.try_wait()?.is_some() {
            // The leader is gone but descendants may remain; still sweep them.
            self.sweep(child);
            return Ok(());
        }
        self.terminate(child);
        let deadline = Instant::now() + REAP_TIMEOUT;
        while Instant::now() < deadline {
            if child.try_wait()?.is_some() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = child.kill();
        child.wait().map(|_| ())
    }

    #[cfg(windows)]
    fn terminate(&self, child: &std::process::Child) {
        match &self.job {
            Some(job) => job.terminate(),
            None => taskkill(child.id()),
        }
    }

    #[cfg(windows)]
    fn sweep(&self, child: &std::process::Child) {
        match &self.job {
            Some(job) => job.terminate(),
            None => taskkill(child.id()),
        }
    }

    #[cfg(unix)]
    fn terminate(&self, child: &mut std::process::Child) {
        let Ok(pgid) = i32::try_from(child.id()) else {
            return;
        };
        // SAFETY: `killpg` takes plain integers and has no memory-safety preconditions. `pgid`
        // is the id of the group this crate created for the child (`process_group(0)`).
        unsafe { libc::killpg(pgid, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.sweep(child);
    }

    #[cfg(unix)]
    fn sweep(&self, child: &std::process::Child) {
        let Ok(pgid) = i32::try_from(child.id()) else {
            return;
        };
        // SAFETY: as above; `ESRCH` (the group is already empty) is ignored.
        unsafe { libc::killpg(pgid, libc::SIGKILL) };
    }

    #[cfg(not(any(unix, windows)))]
    fn terminate(&self, child: &mut std::process::Child) {
        let _ = child.kill();
    }

    #[cfg(not(any(unix, windows)))]
    fn sweep(&self, _child: &std::process::Child) {}
}

/// Fallback when no Job Object could be created or assigned: `taskkill /F /T`.
#[cfg(windows)]
fn taskkill(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(windows)]
mod windows_job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    /// A Job Object handle with `KILL_ON_JOB_CLOSE`, closed on drop.
    pub(super) struct Job(HANDLE);

    impl Job {
        /// Create a job and put `child` in it; `None` if either step fails (for example when an
        /// outer job forbids nesting on a pre-Windows 8 system).
        pub(super) fn create_and_assign(child: &std::process::Child) -> Option<Self> {
            // SAFETY: both arguments are allowed to be null (default security, anonymous job).
            // The result is checked for null before use.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return None;
            }
            let job = Self(handle);
            // SAFETY: the struct is plain old data for which all-zero is a valid value.
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size =
                u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).ok()?;
            // SAFETY: `job.0` is a live job handle owned by `job`; the pointer and size describe
            // `info`, which outlives the call.
            let configured = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&info).cast::<c_void>(),
                    size,
                )
            };
            if configured == 0 {
                return None;
            }
            // SAFETY: `job.0` is a live job handle; `child` owns a live process handle for the
            // duration of the call (`as_raw_handle` borrows it).
            let assigned = unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle()) };
            (assigned != 0).then_some(job)
        }

        /// Kill every process in the job.
        pub(super) fn terminate(&self) {
            // SAFETY: `self.0` is a live job handle owned by `self`.
            unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: `self.0` is a job handle this value owns and closes exactly once. With
            // `KILL_ON_JOB_CLOSE` this also kills any process still in the job.
            unsafe { CloseHandle(self.0) };
        }
    }
}
