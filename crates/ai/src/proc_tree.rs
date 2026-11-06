//! Killing a child process together with everything it started.
//!
//! On Windows `claude` / `codex` are often npm shims (`claude.cmd`): the child we spawn is
//! `cmd.exe` and the real agent (`node.exe`, `codex.exe`, MCP helpers) is a grandchild.
//! `Child::kill` ends only `cmd.exe`; the grandchild keeps running and keeps our stdout pipe
//! open, so a reader waiting for EOF never finishes.
//!
//! [`ProcessTree`] puts the child into a Windows Job Object with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: processes the child starts join the job too, and
//! [`ProcessTree::kill`] (or dropping the tree, or our own process ending) kills all of them.
//! If the job cannot be set up, `kill` falls back to `taskkill /T /F /PID <pid>`.
//!
//! Elsewhere npm shims are scripts run by `node` directly (no extra process), so `kill` does
//! nothing and the caller's `Child::kill` is enough.

/// The process tree of one child. Keep it alive as long as the child runs: dropping it kills
/// the tree on Windows.
pub struct ProcessTree {
    #[cfg(windows)]
    pid: Option<u32>,
    #[cfg(windows)]
    job: Option<win::Job>,
}

impl ProcessTree {
    /// Track the process `pid` (from `Child::id()` right after spawning) and whatever it starts.
    pub fn new(pid: Option<u32>) -> Self {
        #[cfg(windows)]
        {
            let job = pid.and_then(|pid| match win::Job::with_process(pid) {
                Ok(job) => Some(job),
                Err(err) => {
                    tracing::debug!(pid, %err, "no job object; will use taskkill /T");
                    None
                }
            });
            ProcessTree { pid, job }
        }
        #[cfg(not(windows))]
        {
            let _ = pid;
            ProcessTree {}
        }
    }

    /// Kill every process in the tree (the child included). Safe to call more than once.
    pub fn kill(&self) {
        #[cfg(windows)]
        {
            if let Some(job) = &self.job
                && job.terminate()
            {
                return;
            }
            if let Some(pid) = self.pid {
                taskkill_tree(pid);
            }
        }
    }
}

/// Fallback: `taskkill /T /F /PID <pid>` kills the process and its descendants.
#[cfg(windows)]
fn taskkill_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    /// An owned job object handle; closing it kills every process still in the job.
    pub struct Job(HANDLE);

    // The handle is a plain kernel object reference; any thread may use or close it.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        /// A new kill-on-close job containing the process `pid`.
        pub fn with_process(pid: u32) -> std::io::Result<Job> {
            // SAFETY: plain Win32 calls; every pointer passed points to a live local value,
            // and each handle is closed exactly once (`Job` owns the job handle).
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(std::io::Error::last_os_error());
                }
                let job = Job(handle);
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if process.is_null() {
                    return Err(std::io::Error::last_os_error());
                }
                let assigned = AssignProcessToJobObject(job.0, process);
                let err = std::io::Error::last_os_error();
                CloseHandle(process);
                if assigned == 0 {
                    return Err(err);
                }
                Ok(job)
            }
        }

        /// Kill every process in the job. `false` if Windows refused.
        pub fn terminate(&self) -> bool {
            // SAFETY: `self.0` is a valid job handle until `drop`.
            unsafe { TerminateJobObject(self.0, 1) != 0 }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: we own the handle and close it once. KILL_ON_JOB_CLOSE ends the tree.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::time::Duration;

    use tokio::io::AsyncReadExt;

    use super::*;

    /// `cmd /c ping` = cmd.exe with a ping.exe grandchild that shares our stdout pipe, the same
    /// shape as an npm `claude.cmd` shim running node.exe.
    fn spawn_shim_like_tree() -> tokio::process::Child {
        crate::cli::command(std::path::Path::new("cmd.exe"), &std::env::temp_dir())
            .args(["/c", "ping -n 60 127.0.0.1"])
            .spawn()
            .expect("start cmd.exe")
    }

    #[tokio::test]
    async fn kill_ends_grandchildren_so_stdout_closes() {
        let mut child = spawn_shim_like_tree();
        let tree = ProcessTree::new(child.id());
        assert!(tree.job.is_some(), "job object should be available");
        let mut stdout = child.stdout.take().unwrap();
        // Wait until ping runs (it prints its first line).
        let mut buf = [0u8; 64];
        let n = tokio::time::timeout(Duration::from_secs(10), stdout.read(&mut buf))
            .await
            .expect("ping output")
            .unwrap();
        assert!(n > 0);

        tree.kill();
        let _ = child.kill().await;
        // Only ping.exe could keep the pipe open: EOF means it is gone too.
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stdout.read_to_end(&mut rest))
            .await
            .expect("stdout should close once the whole tree is killed")
            .unwrap();
    }

    #[tokio::test]
    async fn taskkill_fallback_kills_the_tree_too() {
        let mut child = spawn_shim_like_tree();
        let mut stdout = child.stdout.take().unwrap();
        let mut buf = [0u8; 64];
        let _ = tokio::time::timeout(Duration::from_secs(10), stdout.read(&mut buf)).await;
        taskkill_tree(child.id().unwrap());
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stdout.read_to_end(&mut rest))
            .await
            .expect("stdout should close after taskkill /T")
            .unwrap();
        let _ = child.wait().await;
    }
}
