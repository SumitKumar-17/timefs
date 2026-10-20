use std::io::{ErrorKind, Read, Write};
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
/// The server handles one client at a time, so a client that connects and then stalls
/// must not hold up other requests or unmount for long.
const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// Requests are one short line; anything longer is rejected rather than buffered.
const MAX_REQUEST: usize = 4096;

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
            Ok((stream, _)) => handle_connection(stream, &shared),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(_) => thread::sleep(POLL),
        }
    }
}

fn handle_connection(stream: UnixStream, shared: &Shared) {
    if stream.set_nonblocking(false).is_err()
        || stream.set_read_timeout(Some(IO_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(IO_TIMEOUT)).is_err()
    {
        return;
    }
    let reply = match read_request_line(&mut &stream, MAX_REQUEST) {
        Ok(line) => match Command::parse(&line) {
            Ok(cmd) => execute(cmd, shared),
            Err(e) => format!("err {e}\n"),
        },
        Err(RequestError::TooLong) => "err request too long\n".to_string(),
        Err(RequestError::NotUtf8) => "err request is not valid UTF-8\n".to_string(),
        // A timed-out or broken client gets no reply; it may not even be reading.
        Err(RequestError::Io(e)) => {
            eprintln!("timefs: dropped control request: {e}");
            return;
        }
    };
    let _ = (&stream).write_all(reply.as_bytes());
}

#[derive(Debug)]
enum RequestError {
    /// No newline within the size limit.
    TooLong,
    /// The line is not UTF-8.
    NotUtf8,
    /// Read failure, including a read timeout.
    Io(std::io::Error),
}

/// Read one request line (up to and excluding `\n`) of at most `limit` bytes. Reading
/// stops at the newline, so bytes after it are left unread. End of input before a
/// newline ends the line, matching a client that writes its request and shuts down.
fn read_request_line(reader: &mut impl Read, limit: usize) -> Result<String, RequestError> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => break,
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) if line.len() >= limit => return Err(RequestError::TooLong),
            Ok(_) => line.push(byte[0]),
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(RequestError::Io(e)),
        }
    }
    String::from_utf8(line).map_err(|_| RequestError::NotUtf8)
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn read(input: &[u8], limit: usize) -> Result<String, RequestError> {
        read_request_line(&mut Cursor::new(input), limit)
    }

    #[test]
    fn reads_one_line_and_stops_at_the_newline() {
        let mut input = Cursor::new(&b"status\nextra"[..]);
        assert_eq!(read_request_line(&mut input, 64).unwrap(), "status");
        assert_eq!(input.position(), 7, "bytes after the newline stay unread");
    }

    #[test]
    fn end_of_input_ends_the_line() {
        assert_eq!(read(b"checkpoint", 64).unwrap(), "checkpoint");
        assert_eq!(read(b"", 64).unwrap(), "");
    }

    #[test]
    fn a_line_at_the_limit_is_accepted_and_one_past_it_rejected() {
        assert_eq!(read(b"abcd\n", 4).unwrap(), "abcd");
        assert!(matches!(read(b"abcde\n", 4), Err(RequestError::TooLong)));
        let endless = vec![b'x'; MAX_REQUEST * 4];
        assert!(matches!(
            read(&endless, MAX_REQUEST),
            Err(RequestError::TooLong)
        ));
    }

    #[test]
    fn invalid_utf8_is_an_error() {
        assert!(matches!(read(b"\xff\n", 64), Err(RequestError::NotUtf8)));
    }

    /// Yields some bytes, then fails like a socket whose read timeout expired.
    struct Stalls(Cursor<Vec<u8>>);

    impl Read for Stalls {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.read(buf)? {
                0 => Err(ErrorKind::WouldBlock.into()),
                n => Ok(n),
            }
        }
    }

    #[test]
    fn a_stalled_client_surfaces_the_timeout() {
        let mut r = Stalls(Cursor::new(b"sta".to_vec()));
        match read_request_line(&mut r, 64) {
            Err(RequestError::Io(e)) => assert_eq!(e.kind(), ErrorKind::WouldBlock),
            other => panic!("expected a timeout error, got {other:?}"),
        }
    }

    #[test]
    fn a_real_socket_read_times_out_instead_of_blocking_forever() {
        let (client, server) = UnixStream::pair().unwrap();
        server
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let started = std::time::Instant::now();
        assert!(matches!(
            read_request_line(&mut &server, 64),
            Err(RequestError::Io(_))
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(client);
    }
}
