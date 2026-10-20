//! LSP's framing: a `Content-Length` header, a blank line, then that many bytes of JSON.

use std::io::{self, BufRead, Write};

/// A message larger than this is taken as a broken stream.
const MESSAGE_LIMIT: usize = 256 * 1024 * 1024;

/// The next message's body, or `None` at the end of the stream.
///
/// # Errors
/// If reading fails or the header is malformed.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut length = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let header = line.trim_end();
        if header.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            let parsed = value
                .trim()
                .parse::<usize>()
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            length = Some(parsed);
        }
    }
    let length = length.unwrap_or(0);
    if length > MESSAGE_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a {length}-byte message"),
        ));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

/// Writes one message.
///
/// # Errors
/// If writing fails (the server went away).
pub fn write_message(writer: &mut impl Write, body: &str) -> io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn messages_should_round_trip_and_end_cleanly() {
        let mut stream = Vec::new();
        write_message(&mut stream, "{\"a\":1}").unwrap();
        write_message(&mut stream, "{\"中\":2}").unwrap();
        let mut reader = Cursor::new(stream);
        assert_eq!(
            (
                read_message(&mut reader).unwrap(),
                read_message(&mut reader).unwrap(),
                read_message(&mut reader).unwrap()
            ),
            (
                Some(b"{\"a\":1}".to_vec()),
                Some("{\"中\":2}".as_bytes().to_vec()),
                None
            )
        );
    }

    #[test]
    fn other_headers_should_be_skipped() {
        let raw = "Content-Type: application/vscode-jsonrpc\r\ncontent-length: 2\r\n\r\n{}";
        let mut reader = Cursor::new(raw.as_bytes().to_vec());
        assert_eq!(read_message(&mut reader).unwrap(), Some(b"{}".to_vec()));
    }
}
