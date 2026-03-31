use serde::Serialize;
use std::io::{self, BufRead, Write};

pub struct MessageReader<R> {
    input: R,
}

impl<R> MessageReader<R>
where
    R: BufRead,
{
    pub fn new(input: R) -> Self {
        Self { input }
    }

    pub fn read_message(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut content_length = None;

        loop {
            let mut line = String::new();
            let bytes_read = self.input.read_line(&mut line)?;
            if bytes_read == 0 {
                return Ok(None);
            }

            if line == "\r\n" {
                break;
            }

            if let Some(value) = line.strip_prefix("Content-Length:") {
                let parsed = value.trim().parse::<usize>().map_err(|err| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid Content-Length header: {err}"),
                    )
                })?;
                content_length = Some(parsed);
            }
        }

        let content_length = content_length.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "missing Content-Length header in LSP message",
            )
        })?;

        let mut payload = vec![0_u8; content_length];
        self.input.read_exact(&mut payload)?;
        Ok(Some(payload))
    }
}

pub struct MessageWriter<W> {
    output: W,
}

impl<W> MessageWriter<W>
where
    W: Write,
{
    pub fn new(output: W) -> Self {
        Self { output }
    }

    pub fn write_json<T>(&mut self, value: &T) -> io::Result<()>
    where
        T: Serialize,
    {
        let payload = serde_json::to_vec(value).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to serialize LSP payload: {err}"),
            )
        })?;

        write!(self.output, "Content-Length: {}\r\n\r\n", payload.len())?;
        self.output.write_all(&payload)?;
        self.output.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::{MessageReader, MessageWriter};
    use std::io::Cursor;

    #[test]
    fn reads_single_framed_message() {
        let input = b"Content-Length: 17\r\n\r\n{\"jsonrpc\":\"2.0\"}";
        let mut reader = MessageReader::new(Cursor::new(input.as_slice()));
        let message = reader.read_message().unwrap().unwrap();
        assert_eq!(message, br#"{"jsonrpc":"2.0"}"#);
    }

    #[test]
    fn writes_single_framed_message() {
        let mut output = Vec::new();
        let mut writer = MessageWriter::new(&mut output);
        writer
            .write_json(&serde_json::json!({ "jsonrpc": "2.0" }))
            .unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.starts_with("Content-Length: "));
        assert!(rendered.ends_with("{\"jsonrpc\":\"2.0\"}"));
    }
}
