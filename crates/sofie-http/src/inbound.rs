use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use hyper::body::{Body, Frame, Incoming, SizeHint};

/// A request's body: read off the connection, or whole, as a socket message carries one, so
/// both pass the same pipeline.
pub(crate) enum Inbound {
    Wire(Incoming),
    #[cfg(feature = "socket")]
    Whole(Option<Bytes>),
}

impl From<Incoming> for Inbound {
    fn from(body: Incoming) -> Self {
        Self::Wire(body)
    }
}

impl Body for Inbound {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, hyper::Error>>> {
        match self.get_mut() {
            Self::Wire(body) => Pin::new(body).poll_frame(context),
            #[cfg(feature = "socket")]
            Self::Whole(bytes) => Poll::Ready(bytes.take().map(|bytes| Ok(Frame::data(bytes)))),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Wire(body) => body.is_end_stream(),
            #[cfg(feature = "socket")]
            Self::Whole(bytes) => bytes.is_none(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match self {
            Self::Wire(body) => body.size_hint(),
            #[cfg(feature = "socket")]
            Self::Whole(bytes) => SizeHint::with_exact(
                bytes
                    .as_ref()
                    .map_or(0, |bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX)),
            ),
        }
    }
}
