use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::{Instant, Sleep, sleep};

/// hyper's header timeout starts only once it has chosen a protocol, so a client
/// that sends nothing, or half an HTTP/2 preface, is cut here.
const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(10);
/// Longer than the request timeout and the live heartbeat, so only a silent peer is cut.
/// It also finds a dead peer, which is why HTTP/2 pings, which would reset it, stay off.
const SILENCE_TIMEOUT: Duration = Duration::from_secs(60);

/// A connection ends once no byte has moved either way for too long, the first byte too.
pub(crate) struct Silenced<I> {
    io: I,
    deadline: Pin<Box<Sleep>>,
}

impl<I> Silenced<I> {
    pub(crate) fn new(io: I) -> Self {
        Self {
            io,
            deadline: Box::pin(sleep(FIRST_BYTE_TIMEOUT)),
        }
    }

    fn heard(&mut self) {
        self.deadline
            .as_mut()
            .reset(Instant::now() + SILENCE_TIMEOUT);
    }

    fn expired(&mut self, context: &mut Context<'_>) -> bool {
        self.deadline.as_mut().poll(context).is_ready()
    }

    fn written(
        &mut self,
        context: &mut Context<'_>,
        written: Poll<io::Result<usize>>,
    ) -> Poll<io::Result<usize>> {
        match written {
            Poll::Ready(Ok(bytes)) if bytes > 0 => {
                self.heard();
                Poll::Ready(Ok(bytes))
            }
            Poll::Pending if self.expired(context) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the client read nothing for too long",
            ))),
            other => other,
        }
    }
}

impl<I: AsyncRead + Unpin> AsyncRead for Silenced<I> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buffer.filled().len();
        match Pin::new(&mut this.io).poll_read(context, buffer) {
            Poll::Ready(Ok(())) if buffer.filled().len() > before => {
                this.heard();
                Poll::Ready(Ok(()))
            }
            // The end of the stream, which hyper takes as the client closing, quietly.
            Poll::Pending if this.expired(context) => Poll::Ready(Ok(())),
            other => other,
        }
    }
}

impl<I: AsyncWrite + Unpin> AsyncWrite for Silenced<I> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let written = Pin::new(&mut this.io).poll_write(context, bytes);
        this.written(context, written)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffers: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let written = Pin::new(&mut this.io).poll_write_vectored(context, buffers);
        this.written(context, written)
    }

    fn is_write_vectored(&self) -> bool {
        self.io.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().io).poll_shutdown(context)
    }
}
