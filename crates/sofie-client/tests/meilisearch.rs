use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use sofie_client::{Meilisearch, Vendor};
use sofie_core::{Uuid, unpoisoned};
use sofie_db::{Document, SearchEngine, SearchIndex};
use tokio::net::TcpListener;

const PRODUCTS: SearchIndex = SearchIndex::new("shop", "product", Some("account"));
const NOTES: SearchIndex = SearchIndex::new("notes", "note", None);
const FOUND: Uuid = Uuid::from_u128(1);

type Asked = Arc<Mutex<Vec<(String, String, Option<String>, Value)>>>;

const SETTINGS_TASK: u64 = 7;
const FAILING_SETTINGS_TASK: u64 = 8;
const FAILED_TASK: u64 = 41;
const SUCCEEDED_TASK: u64 = 42;
const RUNNING_TASK: u64 = 43;
const QUICKLY: Duration = Duration::from_millis(20);

async fn stub() -> (String, Asked) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let asked = Asked::default();
    let recorded = Arc::clone(&asked);
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let recorded = Arc::clone(&recorded);
            let service = service_fn(move |request| answer(request, Arc::clone(&recorded)));
            tokio::spawn(async move {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    (format!("http://{address}"), asked)
}

async fn answer(
    request: Request<Incoming>,
    asked: Asked,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let method = request.method().to_string();
    let target = request.uri().to_string();
    let token = request
        .headers()
        .get("authorization")
        .map(|value| value.to_str().unwrap().to_owned());
    let body = request.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let seen = {
        let mut asked = unpoisoned::lock(&asked);
        asked.push((method, target.clone(), token, body));
        asked.iter().filter(|(_, path, ..)| *path == target).count()
    };
    let (status, answer) = match target.as_str() {
        "/indexes/flaky_index/search" if seen == 1 => (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "message": "Restarting.", "code": "unavailable" }),
        ),
        "/indexes/flaky_index/search" => (StatusCode::OK, json!({ "hits": [] })),
        "/indexes/broken_index/settings" => (
            StatusCode::ACCEPTED,
            json!({ "taskUid": FAILING_SETTINGS_TASK, "status": "enqueued" }),
        ),
        "/tasks/7" => (
            StatusCode::OK,
            json!({ "uid": SETTINGS_TASK, "status": "succeeded" }),
        ),
        "/tasks/8" if seen == 1 => (
            StatusCode::OK,
            json!({ "uid": FAILING_SETTINGS_TASK, "status": "processing" }),
        ),
        "/tasks/8" => (
            StatusCode::OK,
            json!({
                "uid": FAILING_SETTINGS_TASK,
                "status": "failed",
                "error": { "message": "holds `secret words`", "code": "invalid_settings_filterable_attributes" },
            }),
        ),
        "/indexes/shop_product/search" => (
            StatusCode::OK,
            json!({ "hits": [{ "id": FOUND.to_string() }], "query": "wool" }),
        ),
        "/indexes/notes_note/search" => (
            StatusCode::NOT_FOUND,
            json!({ "message": "Index `notes_note` not found.", "code": "index_not_found" }),
        ),
        "/indexes/busy_index/search" => (
            StatusCode::TOO_MANY_REQUESTS,
            json!({ "message": "Too many requests.", "code": "too_many_search_requests" }),
        ),
        "/indexes/watched_index/documents?primaryKey=id" => {
            (StatusCode::ACCEPTED, json!({ "taskUid": FAILED_TASK }))
        }
        "/indexes/watched_index/documents/delete-batch" => {
            (StatusCode::ACCEPTED, json!({ "taskUid": SUCCEEDED_TASK }))
        }
        "/indexes/slow_index/documents?primaryKey=id" => {
            (StatusCode::ACCEPTED, json!({ "taskUid": RUNNING_TASK }))
        }
        listing if listing.starts_with("/tasks?") => (StatusCode::OK, task_list(listing)),
        "/indexes/notes_note/documents/delete-batch" => (
            StatusCode::BAD_REQUEST,
            json!({ "message": "holds `secret words`", "code": "invalid_document_id" }),
        ),
        _ => (
            StatusCode::ACCEPTED,
            json!({ "taskUid": SETTINGS_TASK, "status": "enqueued" }),
        ),
    };
    let mut response = Response::new(Full::new(Bytes::from(answer.to_string())));
    *response.status_mut() = status;
    Ok(response)
}

/// Only the tasks the listing names, as Meilisearch filters by `uids`.
fn task_list(listing: &str) -> Value {
    let tasks = [
        json!({
            "uid": FAILED_TASK,
            "indexUid": "watched_index",
            "status": "failed",
            "type": "documentAdditionOrUpdate",
            "error": {
                "message": "Document holds `secret words`",
                "code": "missing_document_id",
                "type": "invalid_request",
            },
        }),
        json!({ "uid": SUCCEEDED_TASK, "indexUid": "watched_index", "status": "succeeded" }),
        json!({ "uid": RUNNING_TASK, "indexUid": "slow_index", "status": "processing" }),
    ];
    let named = listing
        .strip_prefix("/tasks?uids=")
        .and_then(|query| query.split('&').next())
        .unwrap();
    let named: Vec<&str> = named.split(',').collect();
    let results: Vec<Value> = tasks
        .into_iter()
        .filter(|task| named.contains(&task["uid"].to_string().as_str()))
        .collect();
    json!({ "results": results })
}

fn adapter(base_url: &str, api_key: Option<&str>) -> Meilisearch {
    Meilisearch::new(outgoing(base_url), api_key.map(str::to_owned))
}

fn checking_quickly(base_url: &str) -> Meilisearch {
    Meilisearch::checking_tasks_every(outgoing(base_url), None, QUICKLY)
}

fn outgoing(base_url: &str) -> sofie_client::Outgoing {
    let settings = Vendor {
        base_url: Some(base_url.to_owned()),
        private: Some(true),
        ..Vendor::default()
    };
    sofie_client::for_vendor("meilisearch", settings).unwrap()
}

fn task_checks(asked: &Asked) -> Vec<String> {
    unpoisoned::lock(asked)
        .iter()
        .map(|(_, path, ..)| path.clone())
        .filter(|path| path.starts_with("/tasks?"))
        .collect()
}

async fn until_checked(seen: &Asked, checks: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while task_checks(seen).len() < checks {
        assert!(Instant::now() < deadline, "{checks} task checks are sent");
        tokio::time::sleep(QUICKLY).await;
    }
}

fn scarf() -> Document {
    Document {
        id: FOUND,
        text: "Blue wool scarf".to_owned(),
        owner: None,
    }
}

fn asked(asked: &Asked) -> Vec<(String, String, Option<String>, Value)> {
    unpoisoned::lock(asked).clone()
}

#[tokio::test]
async fn a_match_asks_for_every_word_within_the_owner_and_reads_the_ids() {
    let (url, seen) = stub().await;
    let meilisearch = adapter(&url, Some("search-key"));
    let owner = Uuid::from_u128(9);

    let ids = meilisearch
        .matching(&PRODUCTS, "blue wool", Some(owner))
        .await
        .unwrap();

    assert_eq!(ids, [FOUND]);
    let [(method, path, token, body)] = &asked(&seen)[..] else {
        panic!("one search is sent");
    };
    assert_eq!(
        (method.as_str(), path.as_str()),
        ("POST", "/indexes/shop_product/search")
    );
    assert_eq!(token.as_deref(), Some("Bearer search-key"));
    assert_eq!(
        *body,
        json!({
            "q": "blue wool",
            "limit": 1000,
            "attributesToRetrieve": ["id"],
            "matchingStrategy": "all",
            "filter": format!("owner = \"{owner}\""),
        })
    );
}

#[tokio::test]
async fn an_index_not_made_yet_matches_nothing() {
    let (url, _) = stub().await;

    let ids = adapter(&url, None)
        .matching(&NOTES, "any", None)
        .await
        .unwrap();

    assert_eq!(ids, []);
}

#[tokio::test]
async fn documents_follow_the_index_settings_sent_once_per_index() {
    let (url, seen) = stub().await;
    let meilisearch = adapter(&url, None);
    let owner = Uuid::from_u128(9);
    let document = Document {
        id: FOUND,
        text: "Blue wool scarf".to_owned(),
        owner: Some(owner),
    };

    meilisearch
        .put(&PRODUCTS, std::slice::from_ref(&document))
        .await
        .unwrap();
    meilisearch.put(&PRODUCTS, &[document]).await.unwrap();

    let asked = asked(&seen);
    let requests: Vec<(&str, &str)> = asked
        .iter()
        .map(|(method, path, ..)| (method.as_str(), path.as_str()))
        .collect();
    assert_eq!(
        requests,
        [
            ("PATCH", "/indexes/shop_product/settings"),
            ("GET", "/tasks/7"),
            ("POST", "/indexes/shop_product/documents?primaryKey=id"),
            ("POST", "/indexes/shop_product/documents?primaryKey=id"),
        ]
    );
    assert_eq!(
        asked[0].3,
        json!({ "searchableAttributes": ["text"], "filterableAttributes": ["owner"] })
    );
    assert_eq!(
        asked[2].3,
        json!([{ "id": FOUND.to_string(), "text": "Blue wool scarf", "owner": owner.to_string() }])
    );
}

#[tokio::test]
async fn removing_sends_the_ids_and_a_refusal_names_its_code_not_its_message() {
    let (url, seen) = stub().await;
    let meilisearch = adapter(&url, None);

    meilisearch.remove(&PRODUCTS, &[FOUND]).await.unwrap();
    let refused = meilisearch.remove(&NOTES, &[FOUND]).await.unwrap_err();

    let asked = asked(&seen);
    assert_eq!(asked[0].1, "/indexes/shop_product/documents/delete-batch");
    assert_eq!(asked[0].3, json!([FOUND.to_string()]));
    let refused = refused.to_string();
    assert!(refused.contains("400 (invalid_document_id)"), "{refused}");
    assert!(!refused.contains("secret"), "{refused}");
}

/// Needs `MEILISEARCH_URL` (`http://127.0.0.1:7700` when unset, as `docker run -p 7700:7700
/// getmeili/meilisearch:v1.15` serves) and `MEILISEARCH_API_KEY` if it has a master key.
#[tokio::test]
#[ignore = "needs a running Meilisearch"]
async fn a_real_meilisearch_finds_put_documents_within_their_owner_until_removed() {
    let url = std::env::var("MEILISEARCH_URL").unwrap_or_else(|_| "http://127.0.0.1:7700".into());
    let api_key = std::env::var("MEILISEARCH_API_KEY").ok();
    let meilisearch = adapter(&url, api_key.as_deref());
    let name: &'static str = format!("product_{}", Uuid::now_v7().unwrap())
        .replace('-', "_")
        .leak();
    let index = SearchIndex::new("sofie_test", name, Some("account"));
    let (mine, theirs) = (Uuid::from_u128(1), Uuid::from_u128(2));
    let documents = [
        Document {
            id: Uuid::from_u128(11),
            text: "Blue wool scarf".into(),
            owner: Some(mine),
        },
        Document {
            id: Uuid::from_u128(12),
            text: "Red wool hat".into(),
            owner: Some(theirs),
        },
    ];

    meilisearch.put(&index, &documents).await.unwrap();

    let found = eventually(&meilisearch, &index, Some(mine), 1).await;
    assert_eq!(found, [Uuid::from_u128(11)]);
    assert_eq!(eventually(&meilisearch, &index, None, 2).await.len(), 2);
    meilisearch
        .remove(&index, &[Uuid::from_u128(11)])
        .await
        .unwrap();
    let left = eventually(&meilisearch, &index, Some(mine), 0).await;
    assert_eq!(left, []);
}

async fn eventually(
    meilisearch: &Meilisearch,
    index: &SearchIndex,
    owner: Option<Uuid>,
    expected: usize,
) -> Vec<Uuid> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let found = meilisearch.matching(index, "wool", owner).await.unwrap();
        if found.len() == expected || Instant::now() > deadline {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn a_busy_engine_is_unavailable_rather_than_a_bug() {
    let (url, _) = stub().await;
    let busy = SearchIndex::new("busy", "index", None);

    let error = adapter(&url, None)
        .matching(&busy, "wool", None)
        .await
        .unwrap_err();

    assert!(
        matches!(error, sofie_core::Error::Unavailable(_)),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_search_meeting_a_restarting_engine_is_sent_again() {
    let (url, seen) = stub().await;
    let flaky = SearchIndex::new("flaky", "index", None);

    let ids = adapter(&url, None)
        .matching(&flaky, "wool", None)
        .await
        .unwrap();

    assert_eq!(ids, []);
    assert_eq!(asked(&seen).len(), 2);
}

#[tokio::test]
async fn an_index_whose_settings_task_failed_is_prepared_again_and_its_code_named() {
    let (url, seen) = stub().await;
    let meilisearch = adapter(&url, None);
    let broken = SearchIndex::new("broken", "index", None);
    let document = Document {
        id: FOUND,
        text: "Blue wool scarf".to_owned(),
        owner: None,
    };

    let refused = meilisearch
        .put(&broken, std::slice::from_ref(&document))
        .await
        .unwrap_err()
        .to_string();
    let _ = meilisearch.put(&broken, &[document]).await;

    assert!(
        refused.contains("invalid_settings_filterable_attributes"),
        "{refused}"
    );
    assert!(!refused.contains("secret"), "{refused}");
    let settings = asked(&seen)
        .iter()
        .filter(|(_, path, ..)| path == "/indexes/broken_index/settings")
        .count();
    let documents = asked(&seen)
        .iter()
        .filter(|(_, path, ..)| path.contains("/documents"))
        .count();
    assert_eq!((settings, documents), (2, 0));
}

#[tokio::test]
async fn a_failed_task_is_logged_once_by_its_codes_and_finished_ones_are_no_longer_checked() {
    let (url, seen) = stub().await;
    let telemetry = sofie_telemetry::capture();
    let meilisearch = checking_quickly(&url);
    let watched = SearchIndex::new("watched", "index", None);

    meilisearch.put(&watched, &[scarf()]).await.unwrap();
    meilisearch.remove(&watched, &[FOUND]).await.unwrap();
    until_checked(&seen, 1).await;
    tokio::time::sleep(QUICKLY * 5).await;

    assert_eq!(
        task_checks(&seen),
        [format!(
            "/tasks?uids={FAILED_TASK},{SUCCEEDED_TASK}&limit=200"
        )]
    );
    let failures: Vec<_> = telemetry
        .events()
        .into_iter()
        .filter(|event| event.level == tracing::Level::ERROR)
        .collect();
    let [failure] = &failures[..] else {
        panic!("one failed task is logged: {failures:?}");
    };
    assert_eq!(failure.fields["task_uid"], FAILED_TASK.to_string());
    assert_eq!(failure.fields["index"], "watched_index");
    assert_eq!(failure.fields["error_code"], "missing_document_id");
    assert_eq!(failure.fields["error_type"], "invalid_request");
    assert!(
        failure
            .fields
            .values()
            .all(|value| !value.contains("secret")),
        "{failure:?}"
    );
}

#[tokio::test]
async fn a_task_still_running_is_checked_again() {
    let (url, seen) = stub().await;
    let meilisearch = checking_quickly(&url);
    let slow = SearchIndex::new("slow", "index", None);

    meilisearch.put(&slow, &[scarf()]).await.unwrap();

    until_checked(&seen, 2).await;
}

#[tokio::test]
async fn nothing_is_checked_while_no_task_is_pending() {
    let (url, seen) = stub().await;
    let meilisearch = checking_quickly(&url);

    meilisearch.matching(&PRODUCTS, "wool", None).await.unwrap();
    tokio::time::sleep(QUICKLY * 5).await;

    assert_eq!(task_checks(&seen), Vec::<String>::new());
}
