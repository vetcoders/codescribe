use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use anyhow::Result;

pub struct IpcClient {
    stream: UnixStream,
}

impl IpcClient {
    pub fn connect() -> Result<Self> {
        let socket_path = codescribe_core::ipc::socket_path();
        let stream = UnixStream::connect(socket_path)?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(30)))?;
        Ok(Self { stream })
    }

    pub fn send<C: serde::Serialize, R: serde::de::DeserializeOwned>(
        &mut self,
        cmd: &C,
    ) -> Result<R> {
        let json = serde_json::to_string(cmd)?;
        self.stream.write_all(json.as_bytes())?;
        self.stream.write_all(b"\n")?;
        self.stream.flush()?;

        let mut reader = BufReader::new(&self.stream);
        let mut response = String::new();
        reader.read_line(&mut response)?;

        Ok(serde_json::from_str(&response)?)
    }
}
