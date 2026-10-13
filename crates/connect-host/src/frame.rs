//! The two wire forms of one message.
//!
//! Browser side (native messaging): a 32-bit length in the machine's own
//! byte order, then that many bytes of UTF-8 JSON.
//!
//! App side (the local endpoint): one JSON object per line.
//!
//! Every message is a JSON OBJECT with a string `type`. Anything else is
//! refused here, before the app sees it, and so is anything too large.

use std::io;

use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The largest message the extension may send. The only big one is a
/// site's cookies (the synth keeps at most 400 per site); everything else
/// is a few hundred bytes.
pub const MAX_FROM_BROWSER: usize = 2 * 1024 * 1024;

/// The largest message the app may send to the extension. The browsers
/// drop the connection above 1 MiB; ours are a few hundred bytes.
pub const MAX_TO_BROWSER: usize = 64 * 1024;

/// Message types the helper writes itself; the extension may not send
/// them and the app's lines may not carry them.
pub const RESERVED_PREFIX: &str = "host";

#[derive(Debug)]
pub enum FrameError {
    /// The other side closed cleanly between two messages.
    Closed,
    /// Larger than the limit. The stream cannot be resynchronized.
    TooLarge(usize),
    /// Not a JSON object with a string `type`, or a reserved type.
    Malformed(&'static str),
    Io(io::Error),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Closed => f.write_str("closed"),
            FrameError::TooLarge(n) => write!(f, "a message of {n} bytes is over the limit"),
            FrameError::Malformed(why) => write!(f, "malformed message: {why}"),
            FrameError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> FrameError {
        FrameError::Io(e)
    }
}

/// Check the shape every message must have and return its type.
pub fn message_type(v: &Value) -> Result<&str, FrameError> {
    let obj = v.as_object().ok_or(FrameError::Malformed("not an object"))?;
    let ty = obj
        .get("type")
        .and_then(Value::as_str)
        .ok_or(FrameError::Malformed("no type"))?;
    if ty.is_empty() || ty.len() > 64 {
        return Err(FrameError::Malformed("bad type"));
    }
    Ok(ty)
}

fn is_reserved(ty: &str) -> bool {
    ty == RESERVED_PREFIX || ty.starts_with("host.")
}

/// Parse bytes that came from the browser or from the app into a message
/// that is safe to pass on.
pub fn parse(bytes: &[u8]) -> Result<Value, FrameError> {
    let v: Value = serde_json::from_slice(bytes).map_err(|_| FrameError::Malformed("not JSON"))?;
    if is_reserved(message_type(&v)?) {
        return Err(FrameError::Malformed("reserved type"));
    }
    Ok(v)
}

/// Read one native-messaging message of at most `max` bytes.
pub async fn read_native<R>(r: &mut R, max: usize) -> Result<Value, FrameError>
where
    R: AsyncRead + Unpin,
{
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(FrameError::Closed),
        Err(e) => return Err(e.into()),
    }
    let n = u32::from_ne_bytes(len) as usize;
    if n > max {
        return Err(FrameError::TooLarge(n));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body).await?;
    parse(&body)
}

/// Write one native-messaging message.
pub async fn write_native<W>(w: &mut W, v: &Value) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(v).map_err(|_| FrameError::Malformed("not serializable"))?;
    if body.len() > MAX_TO_BROWSER {
        return Err(FrameError::TooLarge(body.len()));
    }
    w.write_all(&(body.len() as u32).to_ne_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await?;
    Ok(())
}

/// Read one line of at most `max` bytes (the terminator not counted) and
/// return it without the terminator. `Closed` on EOF before any byte.
pub async fn read_line_raw<R>(r: &mut R, max: usize) -> Result<Vec<u8>, FrameError>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    loop {
        let chunk = r.fill_buf().await?;
        if chunk.is_empty() {
            return if line.is_empty() {
                Err(FrameError::Closed)
            } else {
                Err(FrameError::Io(io::ErrorKind::UnexpectedEof.into()))
            };
        }
        let (take, done) = match chunk.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (chunk.len(), false),
        };
        if line.len() + take > max + 1 {
            return Err(FrameError::TooLarge(line.len() + take));
        }
        line.extend_from_slice(&chunk[..take]);
        r.consume(take);
        if done {
            line.pop();
            return Ok(line);
        }
    }
}

/// Read one message from the app side of the helper, or from the helper
/// side of the app.
pub async fn read_line<R>(r: &mut R, max: usize) -> Result<Value, FrameError>
where
    R: AsyncBufRead + Unpin,
{
    let line = read_line_raw(r, max).await?;
    let v: Value = serde_json::from_slice(&line).map_err(|_| FrameError::Malformed("not JSON"))?;
    message_type(&v)?;
    Ok(v)
}

/// Write one message as a line. serde_json never puts a raw line break
/// inside a value, so one message is always exactly one line.
pub async fn write_line<W>(w: &mut W, v: &Value) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
{
    let mut body = serde_json::to_vec(v).map_err(|_| FrameError::Malformed("not serializable"))?;
    body.push(b'\n');
    w.write_all(&body).await?;
    w.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::BufReader;

    fn native(body: &[u8]) -> Vec<u8> {
        let mut out = (body.len() as u32).to_ne_bytes().to_vec();
        out.extend_from_slice(body);
        out
    }

    #[tokio::test]
    async fn native_round_trip() {
        let msg = json!({"type": "hello", "v": 1, "text": "line\nbreak \u{444}"});
        let mut buf = Vec::new();
        write_native(&mut buf, &msg).await.unwrap();
        let mut r = &buf[..];
        assert_eq!(read_native(&mut r, MAX_FROM_BROWSER).await.unwrap(), msg);
        assert!(matches!(read_native(&mut r, MAX_FROM_BROWSER).await, Err(FrameError::Closed)));
    }

    #[tokio::test]
    async fn a_line_is_one_message_even_with_line_breaks_inside() {
        let msg = json!({"type": "x", "text": "a\nb\r\nc"});
        let mut buf = Vec::new();
        write_line(&mut buf, &msg).await.unwrap();
        assert_eq!(buf.iter().filter(|b| **b == b'\n').count(), 1);
        let mut r = BufReader::new(&buf[..]);
        assert_eq!(read_line(&mut r, 1024).await.unwrap(), msg);
        assert!(matches!(read_line(&mut r, 1024).await, Err(FrameError::Closed)));
    }

    #[tokio::test]
    async fn what_is_not_a_typed_object_is_refused() {
        for bad in [
            &b"[1,2]"[..],
            b"\"hello\"",
            b"42",
            b"{}",
            b"{\"type\":7}",
            b"{\"type\":\"\"}",
            b"not json",
            b"",
        ] {
            let framed = native(bad);
            let mut r = &framed[..];
            assert!(
                matches!(read_native(&mut r, MAX_FROM_BROWSER).await, Err(FrameError::Malformed(_))),
                "{:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[tokio::test]
    async fn the_helpers_own_types_cannot_be_sent_by_anyone_else() {
        for ty in ["host", "host.open", "host.anything"] {
            let framed = native(json!({"type": ty}).to_string().as_bytes());
            let mut r = &framed[..];
            assert!(matches!(read_native(&mut r, MAX_FROM_BROWSER).await, Err(FrameError::Malformed(_))), "{ty}");
        }
        // A type that merely starts with the same letters is fine.
        let framed = native(json!({"type": "hostile"}).to_string().as_bytes());
        let mut r = &framed[..];
        assert!(read_native(&mut r, MAX_FROM_BROWSER).await.is_ok());
    }

    #[tokio::test]
    async fn sizes_are_bounded_in_both_forms() {
        // A length prefix over the limit is refused before a byte of the
        // body is read (the body here does not even exist).
        let framed = (1_000_000u32).to_ne_bytes();
        let mut r = &framed[..];
        assert!(matches!(read_native(&mut r, 1000).await, Err(FrameError::TooLarge(1_000_000))));

        let big = json!({"type": "x", "pad": "a".repeat(MAX_TO_BROWSER)});
        let mut buf = Vec::new();
        assert!(matches!(write_native(&mut buf, &big).await, Err(FrameError::TooLarge(_))));
        assert!(buf.is_empty(), "nothing of an oversized message is written");

        let line = format!("{}\n", json!({"type": "x", "pad": "a".repeat(200)}));
        let mut r = BufReader::new(line.as_bytes());
        assert!(matches!(read_line(&mut r, 100).await, Err(FrameError::TooLarge(_))));
    }

    #[tokio::test]
    async fn a_cut_off_message_is_an_error_not_a_clean_close() {
        let mut framed = native(b"{\"type\":\"x\"}");
        framed.truncate(framed.len() - 3);
        let mut r = &framed[..];
        assert!(matches!(read_native(&mut r, MAX_FROM_BROWSER).await, Err(FrameError::Io(_))));
        let mut r = BufReader::new(&b"{\"type\":\"x\"}"[..]);
        assert!(matches!(read_line(&mut r, 100).await, Err(FrameError::Io(_))));
    }
}
