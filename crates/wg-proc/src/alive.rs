//! Whether a process with a given id still runs, for recovery decisions that must not trust a
//! record alone (WG-4285: an adopted verification killed midway leaves its attempt `verifying`).

/// Whether a process with `pid` is running. A process the caller may not query counts as running.
#[must_use]
pub fn process_alive(pid: u32) -> bool {
    alive_impl(pid)
}

#[cfg(unix)]
fn alive_impl(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // Signal 0 checks existence and permission without sending anything.
    let sent = unsafe { libc::kill(pid, 0) };
    sent == 0
        || std::io::Error::last_os_error()
            .raw_os_error()
            .is_some_and(|code| code == libc::EPERM)
}

#[cfg(windows)]
fn alive_impl(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: OpenProcess takes no pointers; a null handle is checked before use.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // The process exists but cannot be opened when access is denied.
        // SAFETY: GetLastError reads the calling thread's last error code.
        return unsafe { windows_sys::Win32::Foundation::GetLastError() } == ERROR_ACCESS_DENIED;
    }
    let mut code: u32 = 0;
    // SAFETY: `handle` is a valid process handle and `code` is a valid out pointer.
    let read = unsafe { GetExitCodeProcess(handle, &mut code) };
    // SAFETY: `handle` was returned by OpenProcess and is closed exactly once.
    unsafe { CloseHandle(handle) };
    read != 0 && code == STILL_ACTIVE as u32
}

#[cfg(test)]
mod tests {
    use super::process_alive;

    #[test]
    fn this_process_is_alive() {
        assert!(process_alive(std::process::id()));
    }

    #[test]
    fn an_exited_child_is_not_alive() {
        let mut child = if cfg!(windows) {
            crate::spawn::Spawn::new("cmd")
                .args(["/C", "exit 0"])
                .spawn()
        } else {
            crate::spawn::Spawn::new("sh")
                .args(["-c", "exit 0"])
                .spawn()
        };
        let Ok(child) = child.as_mut() else {
            panic!("spawn a short-lived child");
        };
        let pid = child.id();
        let _ = child.wait();
        assert!(!process_alive(pid));
    }
}
