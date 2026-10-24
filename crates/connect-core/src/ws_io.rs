//! Byte-stream adapter over a WebSocket.
//!
//! The multiplexer (yamux) wants `futures::io::AsyncRead + AsyncWrite`. A
//! WebSocket carries messages, so this adapter flattens inbound binary
//! messages into bytes and packs outbound bytes into binary messages of at
//! most `WS_MSG_MAX`. Message boundaries carry no meaning for the peer.
//!
//! It also owns link liveness, because it is the only place that sees
//! every inbound frame:
//! - a WebSocket ping goes out every `keepalive`;
//! - if nothing at all (data, ping, pong) arrives for `read_timeout`, the
//!   read fails and the link is torn down and redialed.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use std::time::Duration;

use futures::io::{AsyncRead, AsyncWrite};
use futures::{Sink, Stream};
use tokio::io::{AsyncRead as TokioRead, AsyncWrite as TokioWrite};
use tokio::time::{Instant, Interval, MissedTickBehavior, Sleep};
use tokio_tungstenite::tungstenite::{Bytes, Error as WsError, Message};
use tokio_tungstenite::WebSocketStream;

/// Largest binary message we send. One yamux frame (12 byte header plus at
/// most `SPLIT_SEND` of data) fits.
pub const WS_MSG_MAX: usize = 64 * 1024 + 12;

pub struct WsIo<S> {
    ws: WebSocketStream<S>,
    /// Unread rest of the current inbound message.
    rbuf: Bytes,
    /// Outbound bytes not yet handed to the WebSocket.
    wbuf: Vec<u8>,
    ping: Interval,
    ping_due: bool,
    flush_due: bool,
    read_timeout: Duration,
    read_deadline: Pin<Box<Sleep>>,
}

impl<S> WsIo<S>
where
    S: TokioRead + TokioWrite + Unpin,
{
    pub fn new(ws: WebSocketStream<S>, keepalive: Duration, read_timeout: Duration) -> Self {
        let mut ping = tokio::time::interval_at(Instant::now() + keepalive, keepalive);
        ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
        WsIo {
            ws,
            rbuf: Bytes::new(),
            wbuf: Vec::with_capacity(WS_MSG_MAX),
            ping,
            ping_due: false,
            flush_due: false,
            read_timeout,
            read_deadline: Box::pin(tokio::time::sleep(read_timeout)),
        }
    }

    /// Queue a keepalive ping when one is due and push it out. Called from
    /// `poll_read`, which the multiplexer polls for the whole link lifetime.
    fn drive_keepalive(&mut self, cx: &mut Context<'_>) -> io::Result<()> {
        while self.ping.poll_tick(cx).is_ready() {
            self.ping_due = true;
        }
        if self.ping_due {
            if let Poll::Ready(r) = Pin::new(&mut self.ws).poll_ready(cx) {
                r.map_err(ws_err)?;
                Pin::new(&mut self.ws)
                    .start_send(Message::Ping(Bytes::new()))
                    .map_err(ws_err)?;
                self.ping_due = false;
                self.flush_due = true;
            }
        }
        if self.flush_due {
            if let Poll::Ready(r) = Pin::new(&mut self.ws).poll_flush(cx) {
                r.map_err(ws_err)?;
                self.flush_due = false;
            }
        }
        Ok(())
    }

    /// Hand the buffered outbound bytes to the WebSocket as one message.
    fn poll_send_buf(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.wbuf.is_empty() {
            return Poll::Ready(Ok(()));
        }
        // poll_ready is where WebSocket-level backpressure shows up: it
        // stays pending while the socket cannot take the previous message.
        ready!(Pin::new(&mut self.ws).poll_ready(cx)).map_err(ws_err)?;
        let msg = std::mem::replace(&mut self.wbuf, Vec::with_capacity(WS_MSG_MAX));
        Pin::new(&mut self.ws)
            .start_send(Message::Binary(msg.into()))
            .map_err(ws_err)?;
        Poll::Ready(Ok(()))
    }
}

fn ws_err(e: WsError) -> io::Error {
    match e {
        WsError::Io(e) => e,
        WsError::ConnectionClosed | WsError::AlreadyClosed => {
            io::Error::new(io::ErrorKind::ConnectionAborted, "websocket closed")
        }
        other => io::Error::other(other),
    }
}

impl<S> AsyncRead for WsIo<S>
where
    S: TokioRead + TokioWrite + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        loop {
            if !this.rbuf.is_empty() {
                let n = buf.len().min(this.rbuf.len());
                buf[..n].copy_from_slice(&this.rbuf[..n]);
                this.rbuf = this.rbuf.slice(n..);
                return Poll::Ready(Ok(n));
            }
            this.drive_keepalive(cx)?;
            match Pin::new(&mut this.ws).poll_next(cx) {
                Poll::Ready(Some(Ok(msg))) => {
                    // Anything from the peer proves the link is alive.
                    let deadline = Instant::now() + this.read_timeout;
                    this.read_deadline.as_mut().reset(deadline);
                    match msg {
                        Message::Binary(b) => this.rbuf = b,
                        Message::Close(_) => return Poll::Ready(Ok(0)),
                        // Text is not part of the protocol; ping and pong
                        // are answered by tungstenite itself.
                        _ => {}
                    }
                }
                Poll::Ready(Some(Err(WsError::ConnectionClosed | WsError::AlreadyClosed))) => {
                    return Poll::Ready(Ok(0));
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(ws_err(e))),
                Poll::Ready(None) => return Poll::Ready(Ok(0)),
                Poll::Pending => {
                    if this.read_deadline.as_mut().poll(cx).is_ready() {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!(
                                "nothing received from the synth for {} s",
                                this.read_timeout.as_secs()
                            ),
                        )));
                    }
                    return Poll::Pending;
                }
            }
        }
    }
}

impl<S> AsyncWrite for WsIo<S>
where
    S: TokioRead + TokioWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.wbuf.len() >= WS_MSG_MAX {
            ready!(this.poll_send_buf(cx))?;
        }
        let n = buf.len().min(WS_MSG_MAX - this.wbuf.len());
        this.wbuf.extend_from_slice(&buf[..n]);
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_send_buf(cx))?;
        ready!(Pin::new(&mut this.ws).poll_flush(cx)).map_err(ws_err)?;
        this.flush_due = false;
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_send_buf(cx))?;
        Pin::new(&mut this.ws).poll_close(cx).map_err(ws_err)
    }
}
