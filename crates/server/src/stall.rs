//! A connection wrapper that gives up on peers that hold a connection
//! without using it.
//!
//! Two things no layer above notices:
//!
//! * A client that connects and then **sends nothing** — or nothing past
//!   the start of the HTTP/2 preface. HTTP's own timeouts only start once
//!   the opening bytes have told the server which HTTP version it is
//!   dealing with: HTTP/2 once all of [`H2_PREFACE`] has arrived, HTTP/1 as
//!   soon as a byte departs from it. Until then the connection would wait
//!   forever.
//! * A client that keeps its connection open but **never reads** from it.
//!   Every write then waits forever: the response is never finished, the
//!   stream body is never dropped, and the upstream request behind it stays
//!   open — as does every client WebSocket in the same state. TCP itself
//!   never times such a connection out (the peer is alive, its window is
//!   just closed).
//!
//! [`StallGuard`] fails a read when the peer has not made its HTTP version
//! known by a deadline, and fails a write that has made no progress at all
//! for a set time; either ends the connection like any other I/O error.
//! Neither is a limit on how long a request or a response may take: once
//! its version is known a client may be as quiet as it likes, and every
//! byte the peer accepts starts the write clock afresh.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

/// How an HTTP/2 connection opens. The server reads until the peer's bytes
/// either depart from it (HTTP/1) or include all of it (HTTP/2), with no
/// timeout of its own.
const H2_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// See the module documentation.
pub(crate) struct StallGuard<S> {
    inner: S,
    write_timeout: Duration,
    /// Armed while a write is waiting for the peer.
    stalled_since: Option<Pin<Box<Sleep>>>,
    /// Until the peer's HTTP version is known: when to stop waiting for it.
    undecided_until: Option<Pin<Box<Sleep>>>,
    /// How much of [`H2_PREFACE`] the peer has sent so far.
    preface_matched: usize,
}

impl<S> StallGuard<S> {
    /// Wraps `inner`. The peer has `opening` to make its HTTP version
    /// known; a write blocked for `write_timeout` fails. Both failures are
    /// [`io::ErrorKind::TimedOut`].
    pub(crate) fn new(inner: S, opening: Duration, write_timeout: Duration) -> Self {
        StallGuard {
            inner,
            write_timeout,
            stalled_since: None,
            undecided_until: Some(Box::pin(tokio::time::sleep(opening))),
            preface_matched: 0,
        }
    }

    /// Follows the peer's opening bytes; disarms the opening deadline once
    /// they tell which HTTP version the peer speaks.
    fn opened_with(&mut self, fresh: &[u8]) {
        if self.undecided_until.is_none() {
            return;
        }
        let expected = &H2_PREFACE[self.preface_matched..];
        let compared = fresh.len().min(expected.len());
        if fresh[..compared] != expected[..compared] {
            // HTTP/1: its header timeout takes over.
            self.undecided_until = None;
            return;
        }
        self.preface_matched += compared;
        if self.preface_matched == H2_PREFACE.len() {
            // HTTP/2: its keep-alive pings take over.
            self.undecided_until = None;
        }
    }

    /// Applies the stall rule to the outcome of a write-side operation.
    fn watch<T>(
        &mut self,
        cx: &mut Context<'_>,
        outcome: Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        match outcome {
            Poll::Ready(result) => {
                // Progress (or a real error): the clock stops.
                self.stalled_since = None;
                Poll::Ready(result)
            }
            Poll::Pending => {
                let timeout = self.write_timeout;
                let timer = self
                    .stalled_since
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(timeout)));
                if timer.as_mut().poll(cx).is_ready() {
                    self.stalled_since = None;
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the peer accepted no data for too long",
                    )));
                }
                Poll::Pending
            }
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for StallGuard<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(result) => {
                // Once the peer's version is known, quiet is allowed.
                self.opened_with(&buf.filled()[before..]);
                Poll::Ready(result)
            }
            Poll::Pending => {
                if let Some(deadline) = self.undecided_until.as_mut()
                    && deadline.as_mut().poll(cx).is_ready()
                {
                    self.undecided_until = None;
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the peer connected and never said which HTTP version it speaks",
                    )));
                }
                Poll::Pending
            }
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for StallGuard<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let outcome = Pin::new(&mut self.inner).poll_write(cx, buf);
        self.watch(cx, outcome)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let outcome = Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        self.watch(cx, outcome)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let outcome = Pin::new(&mut self.inner).poll_flush(cx);
        self.watch(cx, outcome)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let outcome = Pin::new(&mut self.inner).poll_shutdown(cx);
        self.watch(cx, outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const OPENING: Duration = Duration::from_secs(120);
    const TIMEOUT: Duration = Duration::from_secs(60);

    fn guard<S>(inner: S) -> StallGuard<S> {
        StallGuard::new(inner, OPENING, TIMEOUT)
    }

    #[tokio::test(start_paused = true)]
    async fn a_write_nobody_reads_times_out() {
        // The pipe holds 16 bytes; nobody reads the other end.
        let (ours, _theirs) = tokio::io::duplex(16);
        let mut guarded = guard(ours);
        let started = tokio::time::Instant::now();
        let error = guarded
            .write_all(&[0u8; 64])
            .await
            .expect_err("the write must give up");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(started.elapsed(), TIMEOUT);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_reader_is_not_a_stalled_one() {
        let (ours, mut theirs) = tokio::io::duplex(16);
        let mut guarded = guard(ours);
        // Reads 16 bytes every 45 seconds: far slower than anyone would
        // like, but never a whole timeout without progress.
        let reader = tokio::spawn(async move {
            let mut total = 0;
            let mut chunk = [0u8; 16];
            while total < 160 {
                tokio::time::sleep(Duration::from_secs(45)).await;
                total += theirs.read(&mut chunk).await.unwrap();
            }
            total
        });
        guarded.write_all(&[7u8; 160]).await.unwrap();
        guarded.flush().await.unwrap();
        assert_eq!(reader.await.unwrap(), 160);
    }

    #[tokio::test(start_paused = true)]
    async fn the_clock_restarts_after_progress() {
        let (ours, mut theirs) = tokio::io::duplex(16);
        let mut guarded = guard(ours);
        // The reader takes one helping after 50 s and then stops for good.
        let reader = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(50)).await;
            let mut chunk = [0u8; 16];
            theirs.read_exact(&mut chunk).await.unwrap();
            // Keep the pipe open without reading.
            tokio::time::sleep(Duration::from_secs(600)).await;
            drop(theirs);
        });
        let started = tokio::time::Instant::now();
        let error = guarded
            .write_all(&[0u8; 64])
            .await
            .expect_err("the write must give up");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        // 50 s until the first progress, then a full timeout from there.
        assert_eq!(started.elapsed(), Duration::from_secs(50) + TIMEOUT);
        reader.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn a_peer_that_never_speaks_is_dropped() {
        let (ours, _theirs) = tokio::io::duplex(64);
        let mut guarded = guard(ours);
        let started = tokio::time::Instant::now();
        let mut byte = [0u8; 1];
        let error = guarded
            .read(&mut byte)
            .await
            .expect_err("the read must give up");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(started.elapsed(), OPENING);
    }

    #[tokio::test(start_paused = true)]
    async fn after_its_first_byte_a_peer_may_be_quiet() {
        let (ours, mut theirs) = tokio::io::duplex(64);
        let mut guarded = guard(ours);
        let writer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(100)).await;
            // Not the HTTP/2 preface: HTTP/1, from the first byte.
            theirs.write_all(b"G").await.unwrap();
            // Ten times the opening allowance of silence, then more.
            tokio::time::sleep(OPENING * 10).await;
            theirs.write_all(b"E").await.unwrap();
            theirs
        });
        let mut got = [0u8; 2];
        guarded.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"GE");
        drop(writer.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn a_peer_that_stops_inside_the_h2_preface_is_dropped() {
        let (ours, mut theirs) = tokio::io::duplex(64);
        let mut guarded = guard(ours);
        // Bytes that could still be HTTP/2 do not tell the version, and
        // more of them do not buy more time.
        let writer = tokio::spawn(async move {
            theirs.write_all(b"P").await.unwrap();
            tokio::time::sleep(Duration::from_secs(100)).await;
            theirs.write_all(b"RI * HT").await.unwrap();
            // Long after the deadline, hang up: a read that gets this far
            // was never given up.
            tokio::time::sleep(OPENING * 10).await;
            drop(theirs);
        });
        let started = tokio::time::Instant::now();
        // Read the way the server tells the version: up to the preface's
        // length, until the bytes depart from it.
        let mut opening = [0u8; H2_PREFACE.len()];
        let mut filled = 0;
        let error = loop {
            match guarded.read(&mut opening[filled..]).await {
                Ok(read) => {
                    assert_ne!(read, 0, "the read must give up before the peer hangs up");
                    filled += read;
                }
                Err(error) => break error,
            }
        };
        assert_eq!(&opening[..filled], b"PRI * HT");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(started.elapsed(), OPENING);
        writer.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn after_the_whole_h2_preface_a_peer_may_be_quiet() {
        let (ours, mut theirs) = tokio::io::duplex(64);
        let mut guarded = guard(ours);
        let writer = tokio::spawn(async move {
            theirs.write_all(&H2_PREFACE[..3]).await.unwrap();
            tokio::time::sleep(Duration::from_secs(100)).await;
            theirs.write_all(&H2_PREFACE[3..]).await.unwrap();
            tokio::time::sleep(OPENING * 10).await;
            theirs.write_all(b"!").await.unwrap();
            theirs
        });
        let mut got = [0u8; H2_PREFACE.len() + 1];
        guarded.read_exact(&mut got).await.unwrap();
        assert_eq!(&got[..H2_PREFACE.len()], H2_PREFACE);
        assert_eq!(got[H2_PREFACE.len()], b'!');
        drop(writer.await.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn a_byte_off_the_h2_preface_makes_it_http1() {
        let (ours, mut theirs) = tokio::io::duplex(64);
        let mut guarded = guard(ours);
        let writer = tokio::spawn(async move {
            // `POST` starts like the preface and departs from it at `O`.
            theirs.write_all(b"P").await.unwrap();
            tokio::time::sleep(Duration::from_secs(100)).await;
            theirs.write_all(b"O").await.unwrap();
            tokio::time::sleep(OPENING * 10).await;
            theirs.write_all(b"ST").await.unwrap();
            theirs
        });
        let mut got = [0u8; 4];
        guarded.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"POST");
        drop(writer.await.unwrap());
    }

    #[tokio::test]
    async fn reads_and_writes_pass_through() {
        let (ours, mut theirs) = tokio::io::duplex(1024);
        let mut guarded = guard(ours);
        guarded.write_all(b"ping").await.unwrap();
        let mut got = [0u8; 4];
        theirs.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"ping");
        theirs.write_all(b"pong").await.unwrap();
        guarded.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"pong");
        guarded.shutdown().await.unwrap();
    }
}
