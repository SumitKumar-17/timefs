use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

/// Send one request line to a running mount and return its full reply.
pub fn request(socket: &Path, line: &str) -> std::io::Result<String> {
    let mut stream = UnixStream::connect(socket)?;
    stream.write_all(line.trim_end().as_bytes())?;
    stream.write_all(b"\n")?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    Ok(reply)
}
