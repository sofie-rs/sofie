//! Against an S3 fake on a local socket that checks every request's signature.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use sofie_core::unpoisoned;
use tokio::net::TcpListener;

use super::*;
use crate::{ContentType, Storage};

const ACCESS_KEY_ID: &str = "AKIDTEST";
const SECRET: &str = "test/secret+key";
const REGION: &str = "eu-central-1";
const BUCKET: &str = "uploads";

#[derive(Default)]
struct Bucket {
    objects: HashMap<String, Vec<u8>>,
    uploads: HashMap<String, Vec<Vec<u8>>>,
    aborted: Vec<String>,
    reads: usize,
    /// Refuses storing this part number, however often it is sent.
    refused_part: Option<usize>,
    ignores_ranges: bool,
}

type Shared = Arc<Mutex<Bucket>>;
type Answer = http::Response<Full<Bytes>>;

fn credentials() -> Credentials {
    Credentials {
        access_key_id: ACCESS_KEY_ID.to_owned(),
        secret_access_key: SECRET.to_owned(),
        region: REGION.to_owned(),
    }
}

fn answer(status: u16, body: impl AsRef<[u8]>) -> Answer {
    let mut response = http::Response::new(Full::new(Bytes::copy_from_slice(body.as_ref())));
    *response.status_mut() = http::StatusCode::from_u16(status).unwrap();
    response
}

fn s3_error(status: u16, code: &str) -> Answer {
    answer(
        status,
        format!("<?xml version=\"1.0\"?><Error><Code>{code}</Code></Error>"),
    )
}

fn decoded(text: &str) -> String {
    String::from_utf8(sofie_core::encoding::percent::decode(text.as_bytes()).unwrap()).unwrap()
}

fn query_of(uri: &http::Uri) -> Vec<(String, String)> {
    uri.query()
        .unwrap_or_default()
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decoded(name), decoded(value))
        })
        .collect()
}

fn timestamp_of(amz_date: &str) -> Timestamp {
    let d = amz_date;
    format!(
        "{}-{}-{}T{}:{}:{}Z",
        &d[0..4],
        &d[4..6],
        &d[6..8],
        &d[9..11],
        &d[11..13],
        &d[13..15]
    )
    .parse()
    .unwrap()
}

/// Recomputed from what arrived, as S3 does.
fn param(query: &[(String, String)], name: &str) -> Option<String> {
    query
        .iter()
        .find(|(found, _)| found == name)
        .map(|(_, value)| value.clone())
}

fn is_signed(parts: &http::request::Parts, body: &[u8]) -> bool {
    let mut query = query_of(&parts.uri);
    if let Some(index) = query.iter().position(|(name, _)| name == "X-Amz-Signature") {
        let (_, signature) = query.remove(index);
        let date = param(&query, "X-Amz-Date").unwrap();
        let expires = param(&query, "X-Amz-Expires").unwrap();
        let extra: Vec<(String, String)> = query
            .iter()
            .filter(|(name, _)| !name.starts_with("X-Amz-"))
            .cloned()
            .collect();
        let host = parts.headers["host"].to_str().unwrap();
        let expected = credentials().presigned_query(
            parts.method.as_str(),
            parts.uri.path(),
            host,
            extra,
            timestamp_of(&date),
            expires.parse().unwrap(),
        );
        return expected.ends_with(&format!("X-Amz-Signature={signature}"));
    }
    let Some(authorization) = parts.headers.get("authorization") else {
        return false;
    };
    let authorization = authorization.to_str().unwrap();
    let signed: Vec<&str> = authorization
        .split("SignedHeaders=")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .unwrap()
        .split(';')
        .collect();
    let headers = signed
        .iter()
        .map(|name| {
            let value = parts.headers[*name].to_str().unwrap().to_owned();
            ((*name).to_owned(), value)
        })
        .collect();
    let payload_hash = parts.headers["x-amz-content-sha256"].to_str().unwrap();
    let date = parts.headers["x-amz-date"].to_str().unwrap();
    let expected = credentials().authorization(
        Canonical {
            method: parts.method.as_str(),
            path: parts.uri.path(),
            query,
            headers,
            payload_hash,
        },
        timestamp_of(date),
    );
    payload_hash == sha256_hex(body) && expected == authorization
}

async fn handle(bucket: Shared, request: http::Request<Incoming>) -> Result<Answer, Infallible> {
    let (parts, body) = request.into_parts();
    let body = body.collect().await.unwrap().to_bytes();
    if !is_signed(&parts, &body) {
        return Ok(s3_error(403, "SignatureDoesNotMatch"));
    }
    let query = query_of(&parts.uri);
    let value = |name: &str| param(&query, name);
    let path = parts.uri.path().to_owned();
    let header = |name: &str| {
        let value = parts.headers.get(name);
        value.map(|value| value.to_str().unwrap().to_owned())
    };
    let (range, if_match) = (header("range"), header("if-match"));
    let mut bucket = unpoisoned::lock(&bucket);
    Ok(match (parts.method.as_str(), value("uploadId")) {
        ("POST", None) => {
            let id = format!("upload-{}", bucket.uploads.len() + 1);
            bucket.uploads.insert(id.clone(), Vec::new());
            answer(
                200,
                format!(
                    "<?xml version=\"1.0\"?><InitiateMultipartUploadResult><UploadId>{id}</UploadId></InitiateMultipartUploadResult>"
                ),
            )
        }
        ("PUT", Some(_))
            if value("partNumber").and_then(|number| number.parse().ok())
                == bucket.refused_part =>
        {
            s3_error(500, "InternalError")
        }
        ("PUT", Some(id)) => {
            let parts = bucket.uploads.get_mut(&id).unwrap();
            parts.push(body.to_vec());
            let mut response = answer(200, "");
            let etag = format!("\"etag-{}\"", parts.len());
            response.headers_mut().insert("etag", etag.parse().unwrap());
            response
        }
        ("POST", Some(id)) => {
            let parts = bucket.uploads.remove(&id).unwrap();
            let listed = String::from_utf8(body.to_vec()).unwrap();
            assert_eq!(listed.matches("<Part>").count(), parts.len());
            assert!(listed.contains(&format!("<ETag>\"etag-{}\"</ETag>", parts.len())));
            bucket.objects.insert(path, parts.concat());
            answer(
                200,
                "<?xml version=\"1.0\"?><CompleteMultipartUploadResult/>",
            )
        }
        ("DELETE", Some(id)) => {
            bucket.uploads.remove(&id);
            bucket.aborted.push(id);
            answer(204, "")
        }
        ("PUT", None) => {
            bucket.objects.insert(path, body.to_vec());
            answer(200, "")
        }
        ("DELETE", None) => {
            bucket.objects.remove(&path);
            answer(204, "")
        }
        ("GET", None) => {
            bucket.reads += 1;
            let range = range.filter(|_| !bucket.ignores_ranges);
            match bucket.objects.get(&path) {
                None => s3_error(404, "NoSuchKey"),
                Some(object) if if_match.is_some_and(|tag| tag != etag_of(object)) => {
                    s3_error(412, "PreconditionFailed")
                }
                Some(object) => ranged(object, range.as_deref()),
            }
        }
        _ => s3_error(400, "InvalidRequest"),
    })
}

fn etag_of(object: &[u8]) -> String {
    format!("\"{}\"", &sha256_hex(object)[..32])
}

fn ranged(object: &[u8], range: Option<&str>) -> Answer {
    let mut response = sliced(object, range);
    let etag = etag_of(object).parse().unwrap();
    response.headers_mut().insert("etag", etag);
    response
}

fn sliced(object: &[u8], range: Option<&str>) -> Answer {
    let Some(range) = range else {
        return answer(200, object);
    };
    let (start, end) = range
        .strip_prefix("bytes=")
        .and_then(|range| range.split_once('-'))
        .unwrap();
    let start: usize = start.parse().unwrap();
    if start >= object.len() {
        return s3_error(416, "InvalidRange");
    }
    let end = end.parse::<usize>().unwrap().min(object.len() - 1);
    let mut response = answer(206, &object[start..=end]);
    let content_range = format!("bytes {start}-{end}/{}", object.len());
    response
        .headers_mut()
        .insert("content-range", content_range.parse().unwrap());
    response
}

async fn fake_s3() -> (String, Shared) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let bucket = Shared::default();
    let serving = Arc::clone(&bucket);
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let bucket = Arc::clone(&serving);
            tokio::spawn(async move {
                let service = service_fn(move |request| handle(Arc::clone(&bucket), request));
                hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await
            });
        }
    });
    (format!("http://{address}"), bucket)
}

fn settings(endpoint: &str) -> S3Settings {
    S3Settings {
        endpoint: Some(endpoint.to_owned()),
        region: REGION.to_owned(),
        bucket: BUCKET.to_owned(),
        access_key_id: ACCESS_KEY_ID.to_owned(),
        secret_access_key: SECRET.to_owned(),
        path_style: true,
    }
}

async fn storage() -> (Storage, Outgoing, Shared) {
    let (endpoint, bucket) = fake_s3().await;
    let (storage, http) = over(settings(&endpoint));
    (storage, http, bucket)
}

fn over(settings: S3Settings) -> (Storage, Outgoing) {
    let http = sofie_client::for_vendor("s3", settings.vendor(true).unwrap()).unwrap();
    let s3 = S3::new(http.clone(), settings).unwrap();
    (Storage::new(s3), http)
}

async fn read_all(storage: &Storage, key: &StorageKey) -> Option<Vec<u8>> {
    let mut content = storage.get(key).await.unwrap()?;
    let mut read = Vec::new();
    while let Some(chunk) = content.chunk().await.unwrap() {
        read.extend_from_slice(&chunk);
    }
    Some(read)
}

fn sample(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).unwrap())
        .collect()
}

#[tokio::test]
async fn stores_reads_and_deletes_a_small_file() {
    let (storage, _, bucket) = storage().await;

    let mut put = storage.put().await.unwrap();
    put.write(Bytes::from_static(b"hello ")).await.unwrap();
    put.write(Bytes::from_static(b"bucket")).await.unwrap();
    let key = put.finish().await.unwrap();

    assert_eq!(read_all(&storage, &key).await.unwrap(), b"hello bucket");
    assert!(storage.exists(&key).await.unwrap());
    assert!(bucket.lock().unwrap().uploads.is_empty());
    storage.delete(&key).await.unwrap();
    assert!(!storage.exists(&key).await.unwrap());
    assert!(read_all(&storage, &key).await.is_none());
}

#[tokio::test]
async fn a_large_file_goes_up_in_parts_and_comes_back_in_ranges() {
    let (storage, _, bucket) = storage().await;
    let file = sample(PART_SIZE + 3 * 1024 * 1024 + 17);

    let mut put = storage.put().await.unwrap();
    for chunk in file.chunks(64 * 1024) {
        put.write(Bytes::copy_from_slice(chunk)).await.unwrap();
    }
    let key = put.finish().await.unwrap();

    let stored = bucket.lock().unwrap().objects[&format!("/{BUCKET}/{key}")].len();
    assert_eq!(stored, file.len());
    assert_eq!(read_all(&storage, &key).await.unwrap(), file);
    assert_eq!(bucket.lock().unwrap().reads, 3);
}

#[tokio::test]
async fn an_opened_download_holds_only_a_small_first_range() {
    let (storage, _, _) = storage().await;
    let mut put = storage.put().await.unwrap();
    put.write(Bytes::from(sample(2 * 1024 * 1024)))
        .await
        .unwrap();
    let key = put.finish().await.unwrap();

    let mut content = storage.get(&key).await.unwrap().unwrap();
    let first = content.chunk().await.unwrap().unwrap();
    let second = content.chunk().await.unwrap().unwrap();

    assert_eq!(content.size(), Some(2 * 1024 * 1024));
    assert_eq!(first.len(), 256 * 1024);
    assert_eq!(second.len(), 2 * 1024 * 1024 - 256 * 1024);
}

async fn opened_two_megabyte_file() -> (crate::Content, Shared, String) {
    let (storage, _, bucket) = storage().await;
    let mut put = storage.put().await.unwrap();
    put.write(Bytes::from(sample(2 * 1024 * 1024)))
        .await
        .unwrap();
    let key = put.finish().await.unwrap();
    let mut content = storage.get(&key).await.unwrap().unwrap();
    content.chunk().await.unwrap().unwrap();
    (content, bucket, format!("/{BUCKET}/{key}"))
}

#[tokio::test]
async fn a_file_overwritten_between_ranges_is_not_read_as_a_mix_of_both() {
    let (mut content, bucket, path) = opened_two_megabyte_file().await;
    bucket
        .lock()
        .unwrap()
        .objects
        .insert(path, sample(3 * 1024 * 1024));

    let error = content.chunk().await.map(drop).unwrap_err().to_string();

    assert!(error.contains("412 PreconditionFailed"), "{error}");
}

#[tokio::test]
async fn a_whole_file_answered_to_a_later_range_is_refused() {
    let (mut content, bucket, _) = opened_two_megabyte_file().await;
    bucket.lock().unwrap().ignores_ranges = true;

    assert!(content.chunk().await.is_err());
}

#[tokio::test]
async fn a_put_whose_part_failed_does_not_finish_without_it() {
    let (storage, _, bucket) = storage().await;
    bucket.lock().unwrap().refused_part = Some(2);
    let mut put = storage.put().await.unwrap();
    put.write(Bytes::from(sample(PART_SIZE))).await.unwrap();

    let written = put.write(Bytes::from(sample(PART_SIZE))).await;
    bucket.lock().unwrap().refused_part = None;
    put.write(Bytes::from(sample(10))).await.ok();
    let finished = put.finish().await;

    assert!(written.is_err());
    assert!(finished.is_err(), "{finished:?}");
    assert!(bucket.lock().unwrap().objects.is_empty());
}

#[tokio::test]
async fn an_empty_file_is_kept_and_read_back_empty() {
    let (storage, _, _) = storage().await;

    let key = storage.put().await.unwrap().finish().await.unwrap();

    assert!(storage.exists(&key).await.unwrap());
    assert_eq!(read_all(&storage, &key).await.unwrap(), b"");
}

#[tokio::test]
async fn an_abandoned_upload_is_aborted() {
    let (storage, _, bucket) = storage().await;
    let mut put = storage.put().await.unwrap();
    put.write(Bytes::from(sample(PART_SIZE))).await.unwrap();

    drop(put);

    for _ in 0..100 {
        if !bucket.lock().unwrap().aborted.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let bucket = bucket.lock().unwrap();
    assert_eq!(bucket.aborted, ["upload-1"]);
    assert!(bucket.uploads.is_empty() && bucket.objects.is_empty());
}

#[tokio::test]
async fn a_presigned_link_downloads_the_file_as_a_safe_type() {
    let (storage, http, _) = storage().await;
    let mut put = storage.put().await.unwrap();
    put.write(Bytes::from_static(b"<html>")).await.unwrap();
    let key = put.finish().await.unwrap();
    let html = ContentType::parse("text/html").unwrap();

    let url = storage
        .download_url(&key, &html, Duration::from_secs(300))
        .unwrap();

    let path = url.split_once("127.0.0.1").unwrap().1;
    let path = &path[path.find('/').unwrap()..];
    assert!(url.starts_with("http://127.0.0.1"));
    assert!(path.contains("response-content-type=application%2Foctet-stream"));
    assert!(path.contains("response-content-disposition=attachment"));
    let downloaded = http.get(path).send().await.unwrap();
    assert_eq!(
        (downloaded.status(), downloaded.body()),
        (200, &b"<html>"[..])
    );
    let forged = path.replace("attachment", "inline");
    assert_eq!(http.get(&forged).send().await.unwrap().status(), 403);
    assert!(
        storage
            .download_url(&key, &html, Duration::from_hours(8 * 24))
            .is_err()
    );
}

#[tokio::test]
async fn a_wrong_secret_is_refused_with_s3_s_code() {
    let (endpoint, _) = fake_s3().await;
    let mut settings = settings(&endpoint);
    settings.secret_access_key = "wrong".to_owned();
    let (storage, _) = over(settings);

    let put = storage.put().await.unwrap();
    let error = put.finish().await.unwrap_err().to_string();

    assert!(error.contains("403 SignatureDoesNotMatch"), "{error}");
}

#[test]
fn a_bucket_is_its_own_host_unless_path_style() {
    let mut settings = settings("https://S3.example.test:443/");
    assert_eq!(settings.base_url().unwrap(), "https://s3.example.test");
    settings.path_style = false;
    settings.endpoint = None;
    assert_eq!(
        settings.base_url().unwrap(),
        "https://uploads.s3.eu-central-1.amazonaws.com"
    );
    settings.bucket = "../etc".to_owned();
    assert!(settings.base_url().is_err());
    settings.bucket = BUCKET.to_owned();
    settings.endpoint = Some("https://s3.example.test/bucket".to_owned());
    assert!(settings.base_url().is_err());
}

#[test]
fn an_error_after_keep_alive_whitespace_is_still_an_error() {
    let padded = b"  \r\n  <?xml version=\"1.0\"?>\n<Error><Code>InternalError</Code></Error>";

    assert!(is_error_document(padded));
    assert!(is_error_document(b"<Error><Code>SlowDown</Code></Error>"));
    assert!(!is_error_document(
        b"<?xml version=\"1.0\"?><CompleteMultipartUploadResult><Key>Error</Key></CompleteMultipartUploadResult>"
    ));
    assert!(!is_error_document(b"<Errors/>"));
}

#[test]
fn an_element_s_entities_are_read() {
    let body = b"<InitiateMultipartUploadResult><UploadId>a&amp;b&lt;c</UploadId></InitiateMultipartUploadResult>";

    assert_eq!(element(body, "UploadId").as_deref(), Some("a&b<c"));
}
