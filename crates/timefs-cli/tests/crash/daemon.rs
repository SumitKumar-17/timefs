//! Running the real `timefs mount` binary as a child process and killing it.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_TIMEOUT: Duration = Duration::from_secs(30);
const MOUNTED_BANNER: &str = "Mounted at";

pub fn timefs(repo: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_timefs"));
    cmd.arg("--repo").arg(repo);
    cmd
}

/// Why FUSE cannot be used here, if it cannot.
pub fn fuse_unavailable() -> Option<String> {
    if !Path::new("/dev/fuse").exists() {
        return Some("/dev/fuse is missing".into());
    }
    match Command::new("fusermount3").arg("-V").output() {
        Ok(out) if out.status.success() => None,
        Ok(out) => Some(format!("fusermount3 -V failed: {}", out.status)),
        Err(e) => Some(format!("fusermount3 not runnable: {e}")),
    }
}

/// A `timefs mount` child process serving `mnt`.
pub struct Daemon {
    child: Option<Child>,
    mnt: PathBuf,
    stderr: Arc<Mutex<String>>,
}

impl Daemon {
    /// Start a mount and wait until it serves requests. On failure returns the daemon's
    /// stderr so the caller can tell a missing FUSE setup from a recovery bug.
    pub fn start(repo: &Path, mnt: &Path, checkpoint_secs: u64) -> Result<Daemon, String> {
        let mut child = timefs(repo)
            .arg("mount")
            .arg(mnt)
            .args(["--checkpoint-interval", &checkpoint_secs.to_string()])
            // Small blocks so modest files still span several extents and chunks.
            .args(["--block-size", "4K"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn failed: {e}"))?;
        let banner = drain_stdout(child.stdout.take().expect("piped"));
        let stderr = drain_stderr(child.stderr.take().expect("piped"));
        let mut daemon = Daemon {
            child: Some(child),
            mnt: mnt.to_path_buf(),
            stderr,
        };
        // The banner is printed only after the kernel mount is in place, so once it
        // arrives the mountpoint is live.
        match banner.recv_timeout(STARTUP_TIMEOUT) {
            Ok(()) => Ok(daemon),
            Err(_) => {
                daemon.kill_and_reap();
                Err(format!("mount did not come up: {}", daemon.stderr()))
            }
        }
    }

    pub fn stderr(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    /// SIGKILL the daemon (no final checkpoint, no cleanup) and detach the dead mount.
    pub fn crash(mut self) {
        self.kill_and_reap();
        lazy_unmount(&self.mnt);
    }

    /// Unmount cleanly and require the daemon to exit successfully, which it does only
    /// when its final checkpoint persisted.
    pub fn unmount(mut self) {
        let out = Command::new("fusermount3")
            .arg("-u")
            .arg(&self.mnt)
            .output()
            .expect("run fusermount3");
        assert!(
            out.status.success(),
            "fusermount3 -u failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let mut child = self.child.take().expect("daemon running");
        let deadline = Instant::now() + EXIT_TIMEOUT;
        let status = loop {
            if let Some(status) = child.try_wait().expect("wait for daemon") {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "daemon did not exit after unmount"
            );
            thread::sleep(Duration::from_millis(20));
        };
        assert!(
            status.success(),
            "daemon exited with {status}: {}",
            self.stderr()
        );
    }

    fn kill_and_reap(&mut self) {
        if let Some(mut child) = self.child.take() {
            // `Child::kill` sends SIGKILL: the daemon gets no chance to checkpoint.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Daemon {
    /// A failing assertion must not leave a dead mount or a stray daemon behind.
    fn drop(&mut self) {
        self.kill_and_reap();
        if is_mounted(&self.mnt) {
            lazy_unmount(&self.mnt);
        }
    }
}

/// Forward a signal on the first "Mounted at" line, and keep reading afterwards so the
/// daemon never blocks or fails on a full or closed stdout pipe.
fn drain_stdout(stdout: impl Read + Send + 'static) -> mpsc::Receiver<()> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if line.starts_with(MOUNTED_BANNER) {
                let _ = tx.send(());
            }
        }
    });
    rx
}

fn drain_stderr(mut stderr: impl Read + Send + 'static) -> Arc<Mutex<String>> {
    let buf = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&buf);
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut chunk) {
            if n == 0 {
                break;
            }
            sink.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&chunk[..n]));
        }
    });
    buf
}

/// A killed FUSE daemon leaves a mount that answers every request with `ENOTCONN`;
/// a lazy unmount detaches it even if the writer still holds files open there.
pub fn lazy_unmount(mnt: &Path) {
    let out = Command::new("fusermount3")
        .arg("-u")
        .arg("-z")
        .arg(mnt)
        .output()
        .expect("run fusermount3");
    assert!(
        out.status.success() || !is_mounted(mnt),
        "lazy unmount of {} failed: {}",
        mnt.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

pub fn is_mounted(mnt: &Path) -> bool {
    let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    let want = mnt.to_string_lossy();
    // Field 5 of mountinfo is the mount point (spaces would be escaped, temp paths have none).
    info.lines()
        .any(|l| l.split(' ').nth(4).is_some_and(|p| p == want))
}
