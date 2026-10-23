use sofie_db::Context as _;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG,
};
use http::{HeaderValue, Method, StatusCode};
use http_body_util::{Either, Full};
use hyper::body::{Body, Frame, Incoming};
use sofie_core::{Error, Result};
use sofie_storage::{Content, SERVED_PATH, Served, Storage, content_disposition};
use tokio::sync::mpsc;

use crate::answer::{self, Response};
use crate::caller::Caller;
use crate::pipeline::Pipeline;
use crate::{Area, RequestContext, Serving};
use sofie_db::Resolve;

const UNSIGNED: &str = "This link is invalid or has expired.";
const READ_AHEAD: usize = 4;

#[cfg(feature = "live")]
type Others = crate::live::Body;
#[cfg(not(feature = "live"))]
type Others = Full<Bytes>;

pub(crate) type Streamed = http::Response<Either<Others, Chunks>>;

impl Serving {
    /// Where uploads go and handlers keep files; the signed links of a `Local` store are served
    /// at `/_sofie/files/`.
    #[must_use]
    pub fn storage(mut self, storage: Storage) -> Self {
        self.storage = Some(storage);
        self
    }
}

impl<A: Area, M> Resolve<RequestContext<A, M>> for Storage {
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        let storage = context
            .matched
            .storage
            .clone()
            .map(|storage| storage.with_clock(context.clock()));
        std::future::ready(storage.ok_or_else(|| {
            Error::internal_message(
                "a handler takes `Storage`, but the app has none; see `Serving::storage`",
            )
        }))
    }
}

/// A download is streamed from the store, so it is answered before the pipeline, whose
/// answers are whole.
pub(crate) async fn answer(
    pipeline: &Pipeline,
    request: http::Request<Incoming>,
    peer: SocketAddr,
) -> Streamed {
    let storage = pipeline.endpoints.serving.storage.as_ref();
    let Some(storage) = storage.filter(|_| is_download(&request)) else {
        #[cfg(feature = "live")]
        let others = crate::live::answer(pipeline, request, peer).await;
        #[cfg(not(feature = "live"))]
        let others = pipeline.handle(request, peer).await;
        return others.map(Either::Left);
    };
    let (request, _) = request.into_parts();
    #[cfg(feature = "shield")]
    if let Some(refusal) = pipeline.shielded(&request, peer) {
        return whole(refusal);
    }
    download(storage, &request).await
}

fn is_download(request: &http::Request<Incoming>) -> bool {
    (request.method() == Method::GET || request.method() == Method::HEAD)
        && request.uri().path().starts_with(SERVED_PATH)
}

async fn download(storage: &Storage, request: &http::request::Parts) -> Streamed {
    let caller = Caller::of(&request.headers);
    let path_and_query = request
        .uri
        .path_and_query()
        .map_or("", http::uri::PathAndQuery::as_str);
    let Some(served) = storage.served(path_and_query) else {
        return whole(answer::refusal(StatusCode::FORBIDDEN, UNSIGNED, caller));
    };
    // A key names one content for good, so the key itself is a strong tag.
    let tag = format!("\"{}\"", served.key);
    if answer::is_held(&request.headers, &tag) {
        let mut response = answer::response(StatusCode::NOT_MODIFIED, None, Bytes::new());
        file_headers(response.headers_mut(), &served, &tag);
        return whole(response);
    }
    let found = if request.method == Method::HEAD {
        let exists = storage.exists(&served.key).await;
        exists.map(|exists| exists.then_some(None))
    } else {
        storage
            .get(&served.key)
            .await
            .map(|content| content.map(Some))
    };
    let content = match found {
        Ok(Some(content)) => content,
        Ok(None) => return whole(answer::failure(Error::NotFound, caller)),
        Err(error) => return whole(answer::failure(error, caller)),
    };
    let mut head = answer::response(StatusCode::OK, None, Bytes::new());
    let headers = head.headers_mut();
    file_headers(headers, &served, &tag);
    let (content_type, disposition) = served.content_type.served();
    if let Ok(value) = HeaderValue::from_str(content_type) {
        headers.insert(CONTENT_TYPE, value);
    }
    let disposition = content_disposition(disposition, served.filename.as_deref());
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        headers.insert(CONTENT_DISPOSITION, value);
    }
    if let Some(size) = content.as_ref().and_then(Content::size) {
        headers.insert(CONTENT_LENGTH, HeaderValue::from(size));
    }
    let (parts, empty) = answer::secure(head).into_parts();
    let body = match content {
        Some(content) => Either::Right(Chunks::reading(content)),
        None => Either::Left(whole_body(empty)),
    };
    http::Response::from_parts(parts, body)
}

/// A stored PDF opens on the app's own origin: sandboxed, it can run nothing there.
fn file_headers(headers: &mut http::HeaderMap, served: &Served, tag: &str) {
    let cache = format!("private, max-age={}, immutable", served.fresh_for.as_secs());
    if let Ok(value) = HeaderValue::from_str(&cache) {
        headers.insert(CACHE_CONTROL, value);
    }
    if let Ok(value) = HeaderValue::from_str(tag) {
        headers.insert(ETAG, value);
    }
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; sandbox"),
    );
}

fn whole(response: Response) -> Streamed {
    answer::secure(response).map(|body| Either::Left(whole_body(body)))
}

fn whole_body(body: Full<Bytes>) -> Others {
    #[cfg(feature = "live")]
    return Either::Left(body);
    #[cfg(not(feature = "live"))]
    body
}

/// Read by a task of its own, so the body only waits on a channel.
pub(crate) struct Chunks {
    receiver: mpsc::Receiver<Result<Bytes>>,
}

impl Chunks {
    fn reading(mut content: Content) -> Self {
        let (sender, receiver) = mpsc::channel(READ_AHEAD);
        tokio::spawn(async move {
            while let Some(chunk) = content.chunk().await.transpose() {
                if let Err(error) = &chunk {
                    tracing::warn!(error.kind = "storage", error = %error, "reading a download failed");
                }
                let failed = chunk.is_err();
                if sender.send(chunk).await.is_err() || failed {
                    return;
                }
            }
        });
        Self { receiver }
    }
}

impl Body for Chunks {
    type Data = Bytes;
    type Error = Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>>>> {
        self.receiver
            .poll_recv(context)
            .map(|chunk| chunk.map(|chunk| chunk.map(Frame::data)))
    }
}
