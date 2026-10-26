//! BREACH needs a secret and attacker-chosen input in one body a victim's browser fetches
//! for another site. CSRF checks read headers, so no token sits in a body, and an answer
//! to a request another site started is never compressed. Static assets hold no secret.

use std::io::Write;

use bytes::Bytes;
use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE, VARY};
use http::request::Parts;
use http::{HeaderMap, HeaderValue, Method};
use http_body_util::{BodyExt, Full};
use hyper::body::Body;

use crate::answer::Response;

/// Below it, a compressed body saves less than the work costs.
const THRESHOLD: u64 = 1024;
const BLOCKING_FROM: usize = 256 * 1024;
const COMPRESSIBLE: [&str; 7] = [
    "text/html",
    "text/css",
    "text/plain",
    "application/json",
    "application/javascript",
    "text/javascript",
    "image/svg+xml",
];
/// Per-request speed over the last percent: quality 4 is about gzip's speed at a smaller size.
const BROTLI_QUALITY: u32 = 4;
const BROTLI_WINDOW: u32 = 22;
const BROTLI_BUFFER: usize = 4096;
const GZIP_LEVEL: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    Brotli,
    Gzip,
}

impl Encoding {
    fn accepted(request: &HeaderMap) -> Option<Self> {
        if acceptance(request, "br") == Some(true) {
            return Some(Self::Brotli);
        }
        (acceptance(request, "gzip") == Some(true)).then_some(Self::Gzip)
    }

    fn name(self) -> &'static str {
        match self {
            Self::Brotli => "br",
            Self::Gzip => "gzip",
        }
    }

    fn compress(self, body: &[u8]) -> std::io::Result<Vec<u8>> {
        match self {
            Self::Brotli => {
                let mut compressed = Vec::new();
                let mut writer = brotli::CompressorWriter::new(
                    &mut compressed,
                    BROTLI_BUFFER,
                    BROTLI_QUALITY,
                    BROTLI_WINDOW,
                );
                writer.write_all(body)?;
                writer.flush()?;
                drop(writer);
                Ok(compressed)
            }
            Self::Gzip => {
                let level = flate2::Compression::new(GZIP_LEVEL);
                let mut encoder = flate2::write::GzEncoder::new(Vec::new(), level);
                encoder.write_all(body)?;
                encoder.finish()
            }
        }
    }
}

/// `Some(false)` when refused with `q=0`, `None` when not named.
fn acceptance(request: &HeaderMap, coding: &str) -> Option<bool> {
    let mut wildcard = None;
    let codings = request
        .get_all(ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','));
    for entry in codings {
        let mut parts = entry.split(';');
        let name = parts.next().unwrap_or_default().trim();
        let accepted = crate::caller::quality(parts) > 0.0;
        if name.eq_ignore_ascii_case(coding) {
            return Some(accepted);
        }
        if name == "*" {
            wildcard = Some(accepted);
        }
    }
    wildcard
}

fn is_cross_site(request: &HeaderMap) -> bool {
    request
        .get("sec-fetch-site")
        .is_some_and(|site| site != "same-origin" && site != "none")
}

fn is_compressible(response: &Response) -> bool {
    let headers = response.headers();
    let Some(content_type) = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let media_type = content_type.split(';').next().unwrap_or_default().trim();
    !headers.contains_key(CONTENT_ENCODING)
        && COMPRESSIBLE.contains(&media_type)
        && response
            .body()
            .size_hint()
            .exact()
            .is_some_and(|length| length >= THRESHOLD)
}

/// A HEAD answer is described as sent unencoded: compressing a body that is then dropped is
/// wasted work, and its encoded length is known only by compressing.
pub(crate) async fn compress(request: &Parts, mut response: Response) -> Response {
    #[cfg(feature = "engine")]
    if let Some(precompressed) = response
        .extensions_mut()
        .remove::<sofie_engine::Precompressed>()
    {
        return with_precompressed(&request.headers, response, precompressed);
    }
    if !is_compressible(&response) {
        return response;
    }
    response.headers_mut().append(
        VARY,
        HeaderValue::from_static("Accept-Encoding, Sec-Fetch-Site"),
    );
    let headers = &request.headers;
    let accepted = Encoding::accepted(headers).filter(|_| !is_cross_site(headers));
    let Some(encoding) = accepted.filter(|_| request.method != Method::HEAD) else {
        return response;
    };
    let (mut parts, body) = response.into_parts();
    let Ok(collected) = body.collect().await;
    let body = collected.to_bytes();
    match compressed(encoding, body.clone()).await {
        Ok(compressed) if compressed.len() < body.len() => {
            let name = HeaderValue::from_static(encoding.name());
            parts.headers.insert(CONTENT_ENCODING, name);
            return Response::from_parts(parts, Full::new(Bytes::from(compressed)));
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(error.kind = "compression", error = %error, "compressing an answer failed; sent as is");
        }
    }
    Response::from_parts(parts, Full::new(body))
}

/// A large answer is compressed off the async workers, which it would hold for milliseconds.
async fn compressed(encoding: Encoding, body: Bytes) -> std::io::Result<Vec<u8>> {
    if body.len() < BLOCKING_FROM {
        return encoding.compress(&body);
    }
    tokio::task::spawn_blocking(move || encoding.compress(&body))
        .await
        .map_err(std::io::Error::other)?
}

#[cfg(feature = "engine")]
fn with_precompressed(
    request: &HeaderMap,
    mut response: Response,
    precompressed: sofie_engine::Precompressed,
) -> Response {
    response
        .headers_mut()
        .append(VARY, HeaderValue::from_static("Accept-Encoding"));
    let Some(encoding) = Encoding::accepted(request) else {
        return response;
    };
    let bytes = match encoding {
        Encoding::Brotli => precompressed.brotli,
        Encoding::Gzip => precompressed.gzip,
    };
    response
        .headers_mut()
        .insert(CONTENT_ENCODING, HeaderValue::from_static(encoding.name()));
    *response.body_mut() = Full::new(Bytes::from_static(bytes));
    response
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use http::StatusCode;

    use super::*;
    use crate::answer::{self, JSON, TEXT};

    fn request(headers: &[(&str, &str)]) -> Parts {
        asked(Method::GET, headers)
    }

    fn asked(method: Method, headers: &[(&str, &str)]) -> Parts {
        let mut builder = http::Request::builder().method(method);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(()).unwrap().into_parts().0
    }

    fn json_of(length: usize) -> Response {
        answer::response(StatusCode::OK, Some(JSON), "x".repeat(length))
    }

    async fn body_of(response: Response) -> Bytes {
        let Ok(collected) = response.into_body().collect().await;
        collected.to_bytes()
    }

    #[test]
    fn brotli_is_preferred_and_a_zero_quality_refuses() {
        let accepted = |value| Encoding::accepted(&request(&[("accept-encoding", value)]).headers);

        assert_eq!(accepted("gzip, deflate, br"), Some(Encoding::Brotli));
        assert_eq!(accepted("gzip, br;q=0"), Some(Encoding::Gzip));
        assert_eq!(accepted("GZIP;q=0.5"), Some(Encoding::Gzip));
        assert_eq!(accepted("*"), Some(Encoding::Brotli));
        assert_eq!(accepted("*, br;q=0"), Some(Encoding::Gzip));
        assert_eq!(accepted("identity"), None);
        assert_eq!(Encoding::accepted(&HeaderMap::new()), None);
    }

    #[tokio::test]
    async fn a_large_text_answer_is_compressed_and_varies_by_encoding() {
        let asked = request(&[("accept-encoding", "br, gzip")]);

        let response = compress(&asked, json_of(4096)).await;

        assert_eq!(response.headers()[CONTENT_ENCODING], "br");
        assert_eq!(response.headers()[VARY], "Accept-Encoding, Sec-Fetch-Site");
        let mut decompressed = String::new();
        brotli::Decompressor::new(&body_of(response).await[..], BROTLI_BUFFER)
            .read_to_string(&mut decompressed)
            .unwrap();
        assert_eq!(decompressed, "x".repeat(4096));
    }

    #[tokio::test]
    async fn an_answer_large_enough_to_compress_off_the_workers_still_arrives_whole() {
        let length = BLOCKING_FROM + 1;

        let response = compress(&request(&[("accept-encoding", "gzip")]), json_of(length)).await;

        let mut decompressed = String::new();
        flate2::read::GzDecoder::new(&body_of(response).await[..])
            .read_to_string(&mut decompressed)
            .unwrap();
        assert_eq!(decompressed.len(), length);
    }

    #[tokio::test]
    async fn a_head_answer_is_left_unencoded_with_the_same_vary() {
        let asked = asked(Method::HEAD, &[("accept-encoding", "br, gzip")]);

        let response = compress(&asked, json_of(4096)).await;

        assert!(!response.headers().contains_key(CONTENT_ENCODING));
        assert_eq!(response.headers()[VARY], "Accept-Encoding, Sec-Fetch-Site");
        assert_eq!(body_of(response).await.len(), 4096);
    }

    #[tokio::test]
    async fn gzip_is_sent_when_brotli_is_not_accepted() {
        let response = compress(&request(&[("accept-encoding", "gzip")]), json_of(4096)).await;

        assert_eq!(response.headers()[CONTENT_ENCODING], "gzip");
        let mut decompressed = String::new();
        flate2::read::GzDecoder::new(&body_of(response).await[..])
            .read_to_string(&mut decompressed)
            .unwrap();
        assert_eq!(decompressed.len(), 4096);
    }

    #[tokio::test]
    async fn small_binary_and_cross_site_answers_are_sent_as_they_are() {
        let accepting = [("accept-encoding", "br")];
        let small = compress(&request(&accepting), json_of(1023)).await;
        let binary = compress(
            &request(&accepting),
            answer::response(StatusCode::OK, Some("image/png"), vec![0; 4096]),
        )
        .await;
        let cross_site = compress(
            &request(&[("accept-encoding", "br"), ("sec-fetch-site", "cross-site")]),
            json_of(4096),
        )
        .await;
        let same_origin = compress(
            &request(&[("accept-encoding", "br"), ("sec-fetch-site", "same-origin")]),
            answer::response(StatusCode::OK, Some(TEXT), "y".repeat(4096)),
        )
        .await;

        assert!(!small.headers().contains_key(CONTENT_ENCODING));
        assert!(!small.headers().contains_key(VARY));
        assert!(!binary.headers().contains_key(CONTENT_ENCODING));
        assert!(!cross_site.headers().contains_key(CONTENT_ENCODING));
        assert!(cross_site.headers().contains_key(VARY));
        assert_eq!(same_origin.headers()[CONTENT_ENCODING], "br");
    }

    #[cfg(feature = "engine")]
    #[tokio::test]
    async fn an_engine_asset_is_sent_as_built_in_the_accepted_encoding() {
        let assets = crate::assets::Assets::embedded();
        let asset = sofie_engine::STYLESHEET;
        let built = asset.precompressed().unwrap();
        let served = || assets.response(&asset.path()).unwrap();

        let brotli = compress(&request(&[("accept-encoding", "gzip, br")]), served()).await;
        let gzip = compress(&request(&[("accept-encoding", "gzip")]), served()).await;
        let identity = compress(&request(&[]), served()).await;

        assert_eq!(brotli.headers()[CONTENT_ENCODING], "br");
        assert_eq!(body_of(brotli).await, built.brotli);
        assert_eq!(gzip.headers()[CONTENT_ENCODING], "gzip");
        assert_eq!(body_of(gzip).await, built.gzip);
        assert_eq!(identity.headers()[VARY], "Accept-Encoding");
        assert!(!identity.headers().contains_key(CONTENT_ENCODING));
        assert_eq!(body_of(identity).await, asset.bytes);
    }
}
