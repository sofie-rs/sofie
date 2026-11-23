//! The browser's socket helper, run in node against a real server: what islands import as
//! `sofie:socket` speaks the protocol `/_sofie/socket` answers.
#![cfg(feature = "slow-tests")]

use std::process::Command;

use serde_json::{Value, json};
use sofie_core::{Cast, ColumnReader, Result, SqlValue};
use sofie_db::{Aggregate, Db, Field, Meta, Outcome, Read, ReadOnly, Reply, Select};
use sofie_http::{App, Routes, Web};
use sofie_testing::{FakeDriver, TestServer};

const HELPER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/web/client/core/socket.js");

struct Note {
    meta: Meta,
    text: String,
}

impl Aggregate for Note {
    const NAME: &'static str = "note";
    const MODULE: &'static str = "notes";
    const FIELDS: &'static [Field] = &[Field::new("text", String::COLUMNS, false)];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.text.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            text: String::decode(row)?,
        })
    }
}

/// The fake database keeps no rows, so each load names itself to tell answers apart.
struct Loaded;

impl Outcome for Loaded {
    fn reply(self) -> Result<Reply> {
        static LOADS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let load = LOADS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Reply::Json(json!({ "load": load })))
    }
}

impl ReadOnly for Loaded {}

async fn list_notes(read: Read) -> Result<Loaded> {
    Note::query().all(&read).await.map(|_| Loaded)
}

async fn write_note() -> Result<Note> {
    Ok(Note {
        meta: Meta::new()?,
        text: "hi".to_owned(),
    })
}

fn routes(routes: &mut Routes<Web>) {
    routes.get("/notes", list_notes);
    routes.post("/notes", write_note);
}

/// Watches `/notes`, writes a note once the first answer arrives, and prints what it heard.
const SCRIPT: &str = r#"
const [helper, url] = process.argv.slice(1);
setTimeout(() => { console.error("timed out"); process.exit(2); }, 5000).unref();
const { Socket } = await import(helper);
const socket = new Socket({ url });
const answers = [];
let written;
await new Promise((resolve) => {
  socket.watch("/notes", async (answer) => {
    answers.push(answer);
    if (answers.length === 1) written = await socket.call("POST", "/notes", { text: "hi" });
    if (answers.length === 2) resolve();
  });
});
const refused = await socket.call("GET", "/nowhere");
console.log(JSON.stringify({ answers, written, refused }));
process.exit(0);
"#;

#[tokio::test]
async fn the_helper_watches_and_calls_a_running_server() {
    let app = App::new(Db::new(FakeDriver::default())).area("notes", routes);
    let server = TestServer::start(app).await;
    let url = format!(
        "{}/_sofie/socket",
        server.origin().replacen("http", "ws", 1)
    );
    let helper = format!("file://{HELPER}");

    let output = tokio::task::spawn_blocking(move || {
        Command::new("node")
            .args(["--input-type=module", "-e", SCRIPT, &helper, &url])
            .output()
            .unwrap()
    })
    .await
    .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let heard: Value = serde_json::from_slice(&output.stdout).unwrap();
    let statuses: Vec<&Value> = heard["answers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|answer| &answer["status"])
        .collect();
    assert_eq!(statuses, [&json!(200), &json!(200)], "{heard}");
    assert_ne!(
        heard["answers"][0]["body"], heard["answers"][1]["body"],
        "{heard}"
    );
    assert_eq!(heard["written"]["status"], 200, "{heard}");
    assert_eq!(heard["refused"]["status"], 404, "{heard}");
}
