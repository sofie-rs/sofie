#![cfg(feature = "uploads")]

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use sofie_core::{Error, Result};
use sofie_db::{Db, Input, InputFields, Outcome, Reply, Resolve, Writes};
use sofie_http::{App, Routes, SigningKeys, Upload, Web};
use sofie_storage::{Local, Storage, StorageKey};
use sofie_testing::{FakeDriver, TestRequest, TestResponse, TestServer};

const BOUNDARY: &str = "----sofie-test-boundary";
const PHOTOS_SCREEN: &str = r#"<Section title="Photos">
  <Form id="new-photo" post="/photos/closed" target="photos">
    <Field name="title" />
  </Form>
</Section>
"#;
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR rest of the image";

struct NewPhoto {
    title: String,
    photo: Upload,
    caption: Option<Upload>,
}

impl Input for NewPhoto {
    fn from_json(value: Value) -> Result<Self> {
        let mut fields = InputFields::new(value)?;
        let title = fields.take("title");
        let photo = fields.take("photo");
        let caption = fields.optional("caption");
        fields.finish()?;
        Ok(Self {
            title: title?,
            photo: photo?,
            caption: caption?,
        })
    }
}

impl<C: Writes> Resolve<C> for NewPhoto {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.input().and_then(Self::from_json))
    }
}

struct Replied(Value);

impl Outcome for Replied {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(self.0))
    }
}

fn described(upload: &Upload, storage: &Storage) -> Result<Value> {
    let link = storage.named_download_url(
        upload.key(),
        upload.content_type(),
        upload.filename(),
        std::time::Duration::from_secs(60),
    )?;
    Ok(json!({
        "key": upload.key(),
        "filename": upload.filename(),
        "content_type": upload.content_type(),
        "size": upload.size(),
        "link": link,
    }))
}

async fn add_photo(input: NewPhoto, storage: Storage) -> Result<Replied> {
    let caption = match &input.caption {
        Some(caption) => described(caption, &storage)?,
        None => Value::Null,
    };
    Ok(Replied(json!({
        "title": input.title,
        "photo": described(&input.photo, &storage)?,
        "caption": caption,
    })))
}

async fn refuse_photo(_input: NewPhoto) -> Result<Replied> {
    Err(Error::Rule("photos are closed".to_owned()))
}

async fn break_on_photo(input: NewPhoto) -> Result<Replied> {
    panic!("the handler broke on {}", input.title)
}

async fn front_page() -> Result<()> {
    Ok(())
}

struct Root(PathBuf);

impl Drop for Root {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

impl Root {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "sofie-http-uploads-{}",
            StorageKey::generate().unwrap()
        )))
    }

    fn storage(&self) -> Storage {
        let keys = Arc::new(SigningKeys::generate().unwrap());
        Storage::new(Local::new(&self.0, keys).unwrap())
    }

    fn files(&self) -> Vec<String> {
        std::fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect()
    }
}

async fn serve() -> (TestServer, Root) {
    let root = Root::new();
    let storage = root.storage();
    let screens = sofie_http::screens_from_sources(&[("shop/web/photos", PHOTOS_SCREEN)]);
    let app = App::new(Db::new(FakeDriver::default()))
        .serving(|serving| serving.storage(storage))
        .screens(screens.unwrap())
        .area("shop", |routes: &mut Routes<Web>| {
            routes
                .post("/photos", add_photo)
                .body_limit(64 * 1024)
                .file_limit(1024);
            routes
                .post("/refused", refuse_photo)
                .body_limit(64 * 1024)
                .file_limit(1024);
            routes
                .post("/albums", add_photo)
                .body_limit(1024 * 1024)
                .file_limit(1024);
            routes.post("/broken", break_on_photo).file_limit(1024);
            routes.post("/avatars", add_photo).file_limit(200 * 1024);
            routes.post("/ping", front_page).file_limit(1024);
            routes.post("/titles", echo_title);
            routes.screen("/photos", "photos", front_page);
            routes
                .post("/photos/closed", refuse_photo)
                .file_limit(1024)
                .then_screen("photos", "photos");
            routes
                .post("/photos/filed", add_photo)
                .file_limit(1024)
                .then_redirect("/photos/{missing}");
            routes.get("/", front_page);
        });
    (TestServer::start(app).await, root)
}

enum Part<'a> {
    Text(&'a str, &'a str),
    File(&'a str, &'a str, &'a str, &'a [u8]),
}

fn multipart(parts: &[Part<'_>]) -> Vec<u8> {
    let mut body = Vec::new();
    for part in parts {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        let (head, data) = match part {
            Part::Text(name, value) => (
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n"),
                value.as_bytes(),
            ),
            Part::File(name, filename, content_type, data) => (
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n\
                     Content-Type: {content_type}\r\n\r\n"
                ),
                *data,
            ),
        };
        body.extend_from_slice(head.as_bytes());
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

async fn post(server: &TestServer, path: &str, parts: &[Part<'_>]) -> TestResponse {
    multipart_request(server.post(path), parts).await
}

fn multipart_request(request: TestRequest, parts: &[Part<'_>]) -> TestRequest {
    request
        .accept_json()
        .header(
            "content-type",
            &format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(multipart(parts))
}

fn field_errors(answer: &TestResponse) -> Value {
    answer.assert_status(422);
    answer.json()
}

#[tokio::test]
async fn a_form_s_files_are_stored_and_reach_the_handler_as_uploads() {
    let (server, root) = serve().await;

    let answer = post(
        &server,
        "/photos",
        &[
            Part::Text("title", "Two words"),
            Part::File("photo", "C:\\pictures\\cat.png", "image/png", PNG),
            Part::File("caption", "caption.txt", "text/plain", b"a cat, sitting"),
        ],
    )
    .await;

    answer.assert_ok();
    let reply = answer.json();
    assert_eq!(reply["title"], "Two words");
    assert_eq!(reply["photo"]["filename"], "cat.png");
    assert_eq!(reply["photo"]["content_type"], "image/png");
    assert_eq!(reply["photo"]["size"], PNG.len());
    let mut stored = root.files();
    stored.sort();
    let mut keys = vec![
        reply["photo"]["key"].as_str().unwrap().to_owned(),
        reply["caption"]["key"].as_str().unwrap().to_owned(),
    ];
    keys.sort();
    assert_eq!(stored, keys);

    let photo = server.get(reply["photo"]["link"].as_str().unwrap()).await;
    photo.assert_ok();
    assert_eq!(photo.header("content-type"), "image/png");
    assert_eq!(
        photo.header("content-disposition"),
        "inline; filename=\"cat.png\"; filename*=UTF-8''cat.png"
    );
    assert_eq!(photo.header("x-content-type-options"), "nosniff");
    assert_eq!(photo.header("content-length"), PNG.len().to_string());
    assert!(
        photo
            .header("cache-control")
            .starts_with("private, max-age=")
    );
    assert_eq!(
        photo.header("content-security-policy"),
        "default-src 'none'; sandbox"
    );
    let link = reply["photo"]["link"].as_str().unwrap();
    let cached = server
        .get(link)
        .header("if-none-match", photo.header("etag"))
        .await;
    assert_eq!(cached.status, http::StatusCode::NOT_MODIFIED);
    let caption = server.get(reply["caption"]["link"].as_str().unwrap()).await;
    assert_eq!(caption.body, "a cat, sitting");
    assert_eq!(caption.header("content-type"), "application/octet-stream");
    assert!(
        caption
            .header("content-disposition")
            .starts_with("attachment; filename=\"caption.txt\"")
    );
}

#[tokio::test]
async fn a_file_field_left_empty_is_absent() {
    let (server, root) = serve().await;

    let answer = post(
        &server,
        "/photos",
        &[
            Part::Text("title", "Only a photo"),
            Part::File("photo", "cat.png", "image/png", PNG),
            Part::File("caption", "", "application/octet-stream", b""),
        ],
    )
    .await;

    answer.assert_ok();
    assert_eq!(answer.json()["caption"], Value::Null);
    assert_eq!(root.files().len(), 1);
}

#[tokio::test]
async fn a_file_not_what_it_claims_or_too_large_is_refused_and_not_kept() {
    let (server, root) = serve().await;

    let disguised = post(
        &server,
        "/photos",
        &[
            Part::Text("title", "Disguised"),
            Part::File(
                "photo",
                "cat.png",
                "image/png",
                b"<script>alert(1)</script>",
            ),
        ],
    )
    .await;
    let large = vec![b'a'; 1025];
    let too_large = post(
        &server,
        "/photos",
        &[
            Part::Text("title", "Large"),
            Part::File("caption", "notes.txt", "text/plain", &large),
            Part::File("photo", "cat.png", "image/png", PNG),
        ],
    )
    .await;

    let refusal = field_errors(&disguised).to_string();
    assert!(
        refusal.contains("photo") && refusal.contains("is not the image/png"),
        "{refusal}"
    );
    let refusal = field_errors(&too_large).to_string();
    assert!(
        refusal.contains("caption") && refusal.contains("at most 1024 bytes"),
        "{refusal}"
    );
    assert_eq!(root.files(), Vec::<String>::new());
}

#[tokio::test]
async fn a_body_over_the_route_s_limit_is_refused() {
    let (server, root) = serve().await;
    let names: Vec<String> = (0..70).map(|index| format!("file_{index}")).collect();
    let data = vec![b'a'; 1000];
    let fields: Vec<Part<'_>> = names
        .iter()
        .map(|name| Part::File(name, "a.txt", "text/plain", &data))
        .collect();

    let answer = post(&server, "/photos", &fields).await;

    answer.assert_status(413);
    assert_eq!(root.files(), Vec::<String>::new());
}

#[tokio::test]
async fn a_file_limit_alone_lets_a_file_that_size_through() {
    let (server, root) = serve().await;
    let mut photo = PNG.to_vec();
    photo.resize(150 * 1024, 0);

    let answer = post(
        &server,
        "/avatars",
        &[
            Part::Text("title", "Large"),
            Part::File("photo", "cat.png", "image/png", &photo),
        ],
    )
    .await;

    answer.assert_ok();
    assert_eq!(root.files().len(), 1);
}

#[tokio::test]
async fn the_files_of_a_refused_request_are_deleted() {
    let (server, root) = serve().await;

    let answer = post(
        &server,
        "/refused",
        &[
            Part::Text("title", "Refused"),
            Part::File("photo", "cat.png", "image/png", PNG),
        ],
    )
    .await;

    answer.assert_status(422);
    assert_eq!(root.files(), Vec::<String>::new());
}

#[tokio::test]
async fn the_files_of_a_committed_action_are_kept_when_what_follows_it_fails() {
    let (server, root) = serve().await;

    let answer = server
        .post("/photos/filed")
        .header(
            "content-type",
            &format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(multipart(&[
            Part::Text("title", "Filed"),
            Part::File("photo", "cat.png", "image/png", PNG),
        ]))
        .await;

    answer.assert_status(500);
    assert_eq!(root.files().len(), 1, "{:?}", root.files());
}

#[tokio::test]
async fn a_plain_body_to_a_route_taking_files_keeps_the_text_limit() {
    let (server, _root) = serve().await;
    let title = "a".repeat(100 * 1024);

    let answer = server
        .post("/albums")
        .accept_json()
        .json(&json!({ "title": title }))
        .await;

    answer.assert_status(413);
}

#[tokio::test]
async fn a_json_body_cannot_name_a_stored_file() {
    let (server, _root) = serve().await;
    let forged = json!({
        "title": "Forged",
        "photo": {
            "key": StorageKey::generate().unwrap(),
            "filename": "cat.png",
            "content_type": "image/png",
            "size": 3,
            "seal": "AAAA",
        },
    });

    let answer = server.post("/photos").accept_json().json(&forged).await;

    let refusal = field_errors(&answer).to_string();
    assert!(
        refusal.contains("is not a file sent with this form"),
        "{refusal}"
    );
}

#[tokio::test]
async fn a_download_link_changed_or_naming_no_file_opens_nothing() {
    let (server, root) = serve().await;
    let answer = post(
        &server,
        "/photos",
        &[
            Part::Text("title", "Linked"),
            Part::File("photo", "cat.png", "image/png", PNG),
        ],
    )
    .await;
    let link = answer.json()["photo"]["link"].as_str().unwrap().to_owned();

    let changed = server.get(&link.replace("image/png", "text/html")).await;
    for file in root.files() {
        std::fs::remove_file(root.0.join(file)).unwrap();
    }
    let gone = server.get(&link).await;

    changed.assert_status(403);
    gone.assert_status(404);
}

async fn eventually_empty(root: &Root) -> bool {
    for _ in 0..100 {
        if root.files().is_empty() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test]
async fn a_page_is_not_sent_files() {
    let (server, root) = serve().await;

    let answer = server
        .get("/")
        .header(
            "content-type",
            &format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(multipart(&[Part::File(
            "photo",
            "cat.png",
            "image/png",
            PNG,
        )]))
        .await;

    answer.assert_status(415);
    assert_eq!(root.files(), Vec::<String>::new());
}

#[tokio::test]
async fn an_action_without_a_file_limit_is_not_sent_files() {
    let (server, root) = serve().await;

    let answer = post(
        &server,
        "/titles",
        &[
            Part::Text("title", "Holiday"),
            Part::File("photo", "cat.png", "image/png", PNG),
        ],
    )
    .await;

    answer.assert_status(415);
    assert_eq!(root.files(), Vec::<String>::new());
}

#[tokio::test]
async fn the_files_of_a_request_that_panicked_are_deleted() {
    let (server, root) = serve().await;

    let answer = post(
        &server,
        "/broken",
        &[
            Part::Text("title", "Broken"),
            Part::File("photo", "cat.png", "image/png", PNG),
        ],
    )
    .await;

    answer.assert_status(500);
    assert!(eventually_empty(&root).await, "{:?}", root.files());
}

#[tokio::test]
async fn a_form_with_files_is_refused_by_an_app_without_storage() {
    let app = App::new(Db::new(FakeDriver::default())).area("shop", |routes: &mut Routes<Web>| {
        routes.post("/photos", add_photo).file_limit(1024);
    });
    let server = TestServer::start(app).await;

    let answer = post(
        &server,
        "/photos",
        &[Part::File("photo", "cat.png", "image/png", PNG)],
    )
    .await;

    answer.assert_status(415);
}

struct Titled(String);

impl Input for Titled {
    fn from_json(value: Value) -> Result<Self> {
        let mut fields = InputFields::new(value)?;
        let title = fields.take("title");
        fields.finish()?;
        Ok(Self(title?))
    }
}

impl<C: Writes> Resolve<C> for Titled {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(context.input().and_then(Self::from_json))
    }
}

async fn echo_title(input: Titled) -> Result<Replied> {
    Ok(Replied(json!({ "title": input.0 })))
}

/// Only a file needs storage; a browser sends a form with a file input as multipart even
/// when no file is chosen.
#[tokio::test]
async fn a_multipart_form_without_files_reaches_an_app_without_storage() {
    let app = App::new(Db::new(FakeDriver::default())).area("shop", |routes: &mut Routes<Web>| {
        routes.post("/titles", echo_title);
    });
    let server = TestServer::start(app).await;

    let answer = post(
        &server,
        "/titles",
        &[
            Part::Text("title", "Holiday"),
            Part::File("photo", "", "application/octet-stream", b""),
        ],
    )
    .await;

    answer.assert_status(200);
    assert_eq!(answer.json(), json!({ "title": "Holiday" }));
}

#[tokio::test]
async fn a_form_with_files_shown_again_keeps_what_was_typed_and_no_file() {
    let (server, _root) = serve().await;
    let parts = [
        Part::Text("title", "Two words"),
        Part::File("photo", "cat.png", "image/png", PNG),
    ];

    let refused = server
        .post("/photos/closed")
        .header(
            "content-type",
            &format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(multipart(&parts))
        .await;

    refused.assert_status(422);
    assert!(
        refused.body.contains("value=\"Two words\""),
        "{}",
        refused.body
    );
    assert!(!refused.body.contains("seal"), "{}", refused.body);
}

#[tokio::test]
async fn a_multipart_body_without_a_boundary_is_a_bad_request() {
    let (server, _root) = serve().await;
    let without_storage = TestServer::start(App::new(Db::new(FakeDriver::default())).area(
        "shop",
        |routes: &mut Routes<Web>| {
            routes.post("/photos", add_photo);
        },
    ))
    .await;

    let page = server
        .get("/")
        .header("content-type", "multipart/form-data")
        .body("x")
        .await;
    let action = without_storage
        .post("/photos")
        .accept_json()
        .header("content-type", "multipart/form-data")
        .body("x")
        .await;

    page.assert_status(400);
    action.assert_status(400);
}

#[tokio::test]
async fn the_files_an_input_does_not_take_are_not_kept() {
    let (server, root) = serve().await;

    let answer = post(
        &server,
        "/ping",
        &[Part::File("photo", "cat.png", "image/png", PNG)],
    )
    .await;

    assert!(answer.status.is_success(), "{}", answer.status);
    assert_eq!(root.files(), Vec::<String>::new());
}

#[cfg(feature = "idempotency")]
#[tokio::test]
async fn a_retried_idempotent_upload_replays_its_first_answer_and_keeps_one_file() {
    let root = Root::new();
    let storage = root.storage();
    let app = sofie_testing::start(
        |db: Db| async move {
            Ok(App::new(db)
                .serving(|serving| serving.storage(storage))
                .area("shop", |routes: &mut Routes<Web>| {
                    routes
                        .post("/photos", add_photo)
                        .idempotent()
                        .file_limit(1024);
                }))
        },
        sofie_db::worker::Worker::new,
    )
    .await;
    let parts = [
        Part::Text("title", "Once"),
        Part::File("photo", "cat.png", "image/png", PNG),
    ];
    let keyed =
        || multipart_request(app.post("/photos"), &parts).header("idempotency-key", "photo-1");

    let first = keyed().await;
    let retried = keyed().await;

    first.assert_ok();
    retried.assert_ok();
    assert_eq!(retried.json(), first.json());
    assert_eq!(root.files().len(), 1, "{:?}", root.files());
}

#[cfg(feature = "jobs")]
#[tokio::test]
async fn a_queue_route_is_not_sent_files() {
    let root = Root::new();
    let storage = root.storage();
    let db = Db::new(FakeDriver::default());
    sofie_db::name_job(&db, "shop.actions.front_page", front_page);
    let app = App::new(db)
        .serving(|serving| serving.storage(storage))
        .area("shop", |routes: &mut Routes<Web>| {
            routes.queue("/exports", front_page);
        });
    let server = TestServer::start(app).await;

    let answer = post(
        &server,
        "/exports",
        &[Part::File("photo", "cat.png", "image/png", PNG)],
    )
    .await;

    answer.assert_status(415);
    assert_eq!(root.files(), Vec::<String>::new());
}
