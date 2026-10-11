//! Stream header protocol and line helpers shared by every stream kind.
//!
//! Every multiplexed stream starts with ONE JSON line from the side that
//! opened it and ONE JSON line in reply:
//!
//! ```text
//! {"kind":"control"}                              opened by this app
//! {"kind":"mcp","server":"cua"}                   opened by the synth
//! {"kind":"tcp","host":"example.com","port":443}
//! {"kind":"rpc"}
//!
//! {"ok":true}   or   {"error":"denied: <reason>"}
//! ```
//!
//! A refusal is never silent: the synth always gets a labeled reason.
//! Policy refusals start with "denied: "; failures (unreachable host,
//! daemon not running) are plain text.

use std::io;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio_util::compat::Compat;

/// One multiplexed stream, readable line by line and writable.
pub type MuxStream = BufReader<Compat<yamux::Stream>>;

pub const MAX_HEADER_LINE: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
pub struct StreamHeader {
    pub kind: String,
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HeaderReply {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ok: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// Why a stream was refused. `Display` gives the exact text the synth sees.
#[derive(Debug)]
pub enum Refusal {
    /// Policy of this app said no. Rendered with the "denied: " prefix.
    Denied(String),
    /// The request was allowed but could not be carried out.
    Failed(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Denied(why) => write!(f, "denied: {why}"),
            Refusal::Failed(why) => f.write_str(why),
        }
    }
}

/// Read one '\n'-terminated line of at most `max` bytes (terminator
/// stripped). `Ok(None)` = clean EOF before any byte.
pub async fn read_line<R>(r: &mut R, max: usize) -> io::Result<Option<Vec<u8>>>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    loop {
        let chunk = r.fill_buf().await?;
        if chunk.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "stream ended in the middle of a line",
            ));
        }
        let (take, done) = match chunk.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (chunk.len(), false),
        };
        if line.len() + take > max + 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("line exceeds {max} bytes"),
            ));
        }
        line.extend_from_slice(&chunk[..take]);
        r.consume(take);
        if done {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some(line));
        }
    }
}

/// Write `v` as one JSON line and flush it.
pub async fn write_json_line<W, T>(w: &mut W, v: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let mut b = serde_json::to_vec(v).map_err(io::Error::other)?;
    b.push(b'\n');
    w.write_all(&b).await?;
    w.flush().await
}

pub async fn reply_ok<W: AsyncWrite + Unpin>(w: &mut W) -> io::Result<()> {
    write_json_line(
        w,
        &HeaderReply {
            ok: true,
            error: String::new(),
        },
    )
    .await
}

/// Answer the header with a refusal and half-close, so the synth reads the
/// reason and then EOF.
pub async fn reply_refusal<S>(s: &mut S, why: &Refusal)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let reply = HeaderReply {
        ok: false,
        error: why.to_string(),
    };
    let _ = write_json_line(s, &reply).await;
    let _ = s.shutdown().await;
}
