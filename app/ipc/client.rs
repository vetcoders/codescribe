use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use super::{IpcCommand, IpcResponse, socket_path};

pub fn send_command_blocking(cmd: &IpcCommand) -> Result<IpcResponse, String> {
    let socket_path = socket_path();
    let mut stream =
        UnixStream::connect(socket_path).map_err(|e| format!("IPC connect failed: {e}"))?;
    let payload = serde_json::to_string(cmd).map_err(|e| e.to_string())?;
    stream
        .write_all(payload.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.write_all(b"\n").map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;

    serde_json::from_str::<IpcResponse>(&line).map_err(|e| e.to_string())
}
