mod tasks;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use sofie_core::{Error, Result, Uuid, unpoisoned};
use sofie_db::{BoxFuture, Document, MATCHES_READ, SearchEngine, SearchIndex};

use crate::{Outgoing, Request, Response, call};

const VENDOR: &str = "meilisearch";

/// A settings task still queued behind others after this is taken as prepared: Meilisearch
/// applies an index's tasks in order, so the documents sent next follow it.
const SETTINGS_CHECKS: u32 = 20;
const SETTINGS_CHECK_PAUSE: Duration = Duration::from_millis(50);

pub struct Meilisearch {
    server: Arc<Server>,
}

struct Server {
    http: Outgoing,
    api_key: Option<String>,
    /// Each settings update is a task Meilisearch queues, so it is sent once per index.
    prepared: Mutex<HashSet<String>>,
    tasks: Mutex<tasks::Pending>,
    check_tasks_every: Duration,
}

impl Meilisearch {
    #[must_use]
    pub fn new(http: Outgoing, api_key: Option<String>) -> Self {
        Self::built(http, api_key, tasks::CHECK_EVERY)
    }

    #[cfg(feature = "testing")]
    #[must_use]
    pub fn checking_tasks_every(http: Outgoing, api_key: Option<String>, every: Duration) -> Self {
        Self::built(http, api_key, every)
    }

    fn built(http: Outgoing, api_key: Option<String>, every: Duration) -> Self {
        Self {
            server: Arc::new(Server {
                http,
                api_key,
                prepared: Mutex::default(),
                tasks: Mutex::default(),
                check_tasks_every: every,
            }),
        }
    }
}

impl Server {
    fn authorized<'r>(&self, request: Request<'r>) -> Request<'r> {
        match &self.api_key {
            Some(key) => request.bearer(key),
            None => request,
        }
    }

    async fn search(
        &self,
        index: &SearchIndex,
        words: &str,
        owner: Option<Uuid>,
    ) -> Result<Vec<Uuid>> {
        let mut body = json!({
            "q": words,
            "limit": MATCHES_READ,
            "attributesToRetrieve": ["id"],
            "matchingStrategy": "all",
        });
        if let Some(owner) = owner {
            body["filter"] = Value::String(format!("owner = \"{owner}\""));
        }
        let path = format!("/indexes/{}/search", index.uid());
        let response = self
            .authorized(self.http.post(&path))
            .json(&body)
            .repeatable()
            .send()
            .await?;
        if response.status() == 404 && error_code(&response).as_deref() == Some("index_not_found") {
            return Ok(Vec::new());
        }
        let answer: Value = accepted(response)?.json()?;
        answer
            .get("hits")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::internal_message("meilisearch answered a search without hits"))?
            .iter()
            .map(hit_id)
            .collect()
    }

    async fn add(self: &Arc<Self>, index: &SearchIndex, documents: &[Document]) -> Result<()> {
        self.prepare(index).await?;
        let documents: Vec<Value> = documents
            .iter()
            .map(|document| {
                let mut fields = json!({ "id": document.id, "text": document.text });
                if index.owned() {
                    fields["owner"] = json!(document.owner);
                }
                fields
            })
            .collect();
        let path = format!("/indexes/{}/documents?primaryKey=id", index.uid());
        let request = self.authorized(self.http.post(&path)).json(&documents);
        self.enqueued(request).await
    }

    async fn delete(self: &Arc<Self>, index: &SearchIndex, ids: &[Uuid]) -> Result<()> {
        let path = format!("/indexes/{}/documents/delete-batch", index.uid());
        let request = self.authorized(self.http.post(&path)).json(ids);
        self.enqueued(request).await
    }

    async fn enqueued(self: &Arc<Self>, request: Request<'_>) -> Result<()> {
        let answer: Value = accepted(request.send().await?)?.json()?;
        if let Some(task) = answer.get("taskUid").and_then(Value::as_u64) {
            self.watch(task);
        }
        Ok(())
    }

    /// Updating the settings of an index not there yet makes it. Remembered only once the task is
    /// accepted and has not failed, so a refused one is sent again with the next documents.
    async fn prepare(&self, index: &SearchIndex) -> Result<()> {
        let uid = index.uid();
        if self.prepared().contains(&uid) {
            return Ok(());
        }
        let filterable: &[&str] = if index.owned() { &["owner"] } else { &[] };
        let settings = json!({
            "searchableAttributes": ["text"],
            "filterableAttributes": filterable,
        });
        let path = format!("/indexes/{uid}/settings");
        let request = self.authorized(self.http.patch(&path)).json(&settings);
        let task: Value = accepted(request.send().await?)?.json()?;
        if let Some(task) = task.get("taskUid").and_then(Value::as_u64) {
            self.settled(task).await?;
        }
        self.prepared().insert(uid);
        Ok(())
    }

    /// A key without `tasks.get` cannot see the task, which then counts as accepted.
    async fn settled(&self, task: u64) -> Result<()> {
        let path = format!("/tasks/{task}");
        for _ in 0..SETTINGS_CHECKS {
            let response = self.authorized(self.http.get(&path)).send().await?;
            if matches!(response.status(), 401 | 403) {
                return Ok(());
            }
            let answer: Value = accepted(response)?.json()?;
            match answer.get("status").and_then(Value::as_str) {
                Some("enqueued" | "processing") => tokio::time::sleep(SETTINGS_CHECK_PAUSE).await,
                Some("failed" | "canceled") => {
                    let code = answer
                        .pointer("/error/code")
                        .and_then(Value::as_str)
                        .unwrap_or("no code");
                    return Err(Error::internal_message(format!(
                        "meilisearch refused the settings of an index ({code})"
                    )));
                }
                _ => return Ok(()),
            }
        }
        Ok(())
    }

    fn prepared(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        unpoisoned::lock(&self.prepared)
    }
}

impl SearchEngine for Meilisearch {
    fn matching<'a>(
        &'a self,
        index: &'a SearchIndex,
        words: &'a str,
        owner: Option<Uuid>,
    ) -> BoxFuture<'a, Result<Vec<Uuid>>> {
        Box::pin(call(
            VENDOR,
            "matching",
            self.server.search(index, words, owner),
        ))
    }

    fn put<'a>(
        &'a self,
        index: &'a SearchIndex,
        documents: &'a [Document],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(call(VENDOR, "put", self.server.add(index, documents)))
    }

    fn remove<'a>(&'a self, index: &'a SearchIndex, ids: &'a [Uuid]) -> BoxFuture<'a, Result<()>> {
        Box::pin(call(VENDOR, "remove", self.server.delete(index, ids)))
    }
}

/// Names Meilisearch's error code, never its message, which may quote a document.
fn accepted(response: Response) -> Result<Response> {
    let status = response.status();
    if (200..300).contains(&status) {
        return Ok(response);
    }
    let code = error_code(&response).unwrap_or_else(|| "no code".to_owned());
    let problem = format!("meilisearch answered {status} ({code})");
    Err(if status >= 500 || status == 429 {
        Error::Unavailable(problem)
    } else {
        Error::internal_message(problem)
    })
}

fn error_code(response: &Response) -> Option<String> {
    let body: Value = serde_json::from_slice(response.body()).ok()?;
    Some(body.get("code")?.as_str()?.to_owned())
}

fn hit_id(hit: &Value) -> Result<Uuid> {
    hit.get("id")
        .and_then(Value::as_str)
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| Error::internal_message("meilisearch answered a hit whose id is no id"))
}
