use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::protocol::Command;
use crate::engine::SnapshotOutcome;
use crate::shared::Shared;

const POLL: Duration = Duration::from_millis(50);

pub struct ControlServer {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ControlServer {
    /// Listen on `path`. The caller holds the repository lock, so a leftover socket file
    /// from a crashed daemon is safe to replace.
    pub fn start(path: &Path, shared: Arc<Shared>) -> std::io::Result<Self> {
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("timefs-control".into())
            .spawn(move || serve(listener, shared, flag))?;
        Ok(ControlServer {
            path: path.to_path_buf(),
            stop,
            handle: Some(handle),
        })
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn serve(listener: UnixListener, shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                handle_connection(stream, &shared);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(_) => thread::sleep(POLL),
        }
    }
}

fn handle_connection(stream: UnixStream, shared: &Shared) {
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let reply = match Command::parse(&line) {
        Ok(cmd) => execute(cmd, shared),
        Err(e) => format!("err {e}\n"),
    };
    let _ = (&stream).write_all(reply.as_bytes());
}

fn execute(cmd: Command, shared: &Shared) -> String {
    match cmd {
        Command::Snap { message } => match shared.snapshot(&message) {
            Ok(SnapshotOutcome::Created(h)) => format!("ok created {h}\n"),
            Ok(SnapshotOutcome::Unchanged(h)) => format!("ok unchanged {h}\n"),
            Err(e) => format!("err {e}\n"),
        },
        Command::Checkpoint => match shared.checkpoint() {
            Ok(Some(h)) => format!("ok checkpoint {h}\n"),
            Ok(None) => "ok nothing to checkpoint\n".to_string(),
            Err(e) => format!("err {e}\n"),
        },
        Command::Status => {
            let wt = shared.lock();
            format!(
                "ok\nbranch {}\ndirty {}\ndirty_bytes {}\ninodes {}\n",
                shared.engine().branch(),
                wt.is_dirty(),
                wt.dirty_bytes(),
                wt.node_count()
            )
        }
    }
}
