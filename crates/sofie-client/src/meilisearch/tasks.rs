//! Meilisearch answers a document write once it is queued; the task may fail later, unseen by
//! the job that sent it. Its uid is kept in this process only: one lost on a restart costs no
//! more than the log line of a task that failed.

use std::collections::BTreeSet;
use std::sync::{Arc, MutexGuard, Weak};
use std::time::Duration;

use serde_json::Value;
use sofie_core::{Error, Result, unpoisoned};

use super::{Server, VENDOR, accepted};
use crate::call;

pub(super) const CHECK_EVERY: Duration = Duration::from_secs(30);
const MOST_PENDING: usize = 10_000;
const UIDS_PER_CHECK: usize = 200;

#[derive(Default)]
pub(super) struct Pending {
    uids: BTreeSet<u64>,
    dropped: usize,
    checking: bool,
    unreadable: bool,
}

impl Pending {
    /// Whether no check runs yet, so one must start.
    fn add(&mut self, uid: u64) -> bool {
        if self.unreadable {
            return false;
        }
        if self.uids.len() >= MOST_PENDING {
            self.dropped += 1;
            return false;
        }
        self.uids.insert(uid);
        !std::mem::replace(&mut self.checking, true)
    }

    fn batches(&self) -> Vec<Vec<u64>> {
        let uids: Vec<u64> = self.uids.iter().copied().collect();
        uids.chunks(UIDS_PER_CHECK).map(<[u64]>::to_vec).collect()
    }

    /// Whether the check goes on; decided under the lock `add` takes, so no uid waits unchecked.
    fn keeps_checking(&mut self) -> bool {
        self.checking = !self.uids.is_empty();
        self.checking
    }
}

enum Answer {
    Tasks(Vec<Value>),
    Unreadable,
}

impl Server {
    pub(super) fn watch(self: &Arc<Self>, uid: u64) {
        if self.pending().add(uid) {
            tokio::spawn(check_until_done(
                Arc::downgrade(self),
                self.check_tasks_every,
            ));
        }
    }

    fn pending(&self) -> MutexGuard<'_, Pending> {
        unpoisoned::lock(&self.tasks)
    }

    async fn check_tasks(&self) {
        let dropped = std::mem::take(&mut self.pending().dropped);
        if dropped > 0 {
            tracing::warn!(
                dropped,
                most_pending = MOST_PENDING,
                "meilisearch tasks were not watched: too many were pending"
            );
        }
        let batches = self.pending().batches();
        for uids in batches {
            match call(VENDOR, "tasks", self.tasks_of(&uids)).await {
                Ok(Answer::Tasks(tasks)) => self.settle(&uids, &tasks),
                Ok(Answer::Unreadable) => {
                    tracing::warn!(
                        "the meilisearch key cannot read tasks, so failed indexing is not logged"
                    );
                    let mut pending = self.pending();
                    pending.unreadable = true;
                    pending.uids.clear();
                    return;
                }
                Err(error) => {
                    tracing::warn!(error = %error, "checking meilisearch tasks failed");
                    return;
                }
            }
        }
    }

    async fn tasks_of(&self, uids: &[u64]) -> Result<Answer> {
        let listed: Vec<String> = uids.iter().map(u64::to_string).collect();
        let path = format!("/tasks?uids={}&limit={UIDS_PER_CHECK}", listed.join(","));
        let response = self.authorized(self.http.get(&path)).send().await?;
        if matches!(response.status(), 401 | 403) {
            return Ok(Answer::Unreadable);
        }
        let mut answer: Value = accepted(response)?.json()?;
        match answer.get_mut("results").map(Value::take) {
            Some(Value::Array(tasks)) => Ok(Answer::Tasks(tasks)),
            _ => Err(Error::internal_message(
                "meilisearch answered a task list without results",
            )),
        }
    }

    /// A uid Meilisearch no longer lists was pruned from its task history: nothing left to learn.
    fn settle(&self, uids: &[u64], tasks: &[Value]) {
        let mut running = BTreeSet::new();
        for task in tasks {
            match task.get("status").and_then(Value::as_str) {
                Some("enqueued" | "processing") => {
                    running.extend(task.get("uid").and_then(Value::as_u64));
                }
                Some("failed" | "canceled") => log_failed(task),
                _ => {}
            }
        }
        let mut pending = self.pending();
        for uid in uids.iter().filter(|uid| !running.contains(uid)) {
            pending.uids.remove(uid);
        }
    }
}

async fn check_until_done(server: Weak<Server>, every: Duration) {
    loop {
        tokio::time::sleep(every).await;
        let Some(server) = server.upgrade() else {
            return;
        };
        server.check_tasks().await;
        if !server.pending().keeps_checking() {
            return;
        }
    }
}

/// Meilisearch's error message is left out: it may quote a document.
fn log_failed(task: &Value) {
    let text = |pointer: &str| task.pointer(pointer).and_then(Value::as_str).unwrap_or("");
    tracing::error!(
        task_uid = %task["uid"],
        index = text("/indexUid"),
        task_type = text("/type"),
        status = text("/status"),
        error_code = text("/error/code"),
        error_type = text("/error/type"),
        "a meilisearch task failed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_first_pending_uid_starts_a_check_and_the_cap_counts_what_it_drops() {
        let mut pending = Pending::default();

        let started: Vec<bool> = (0..=MOST_PENDING as u64)
            .map(|uid| pending.add(uid))
            .collect();

        assert_eq!(started.iter().filter(|started| **started).count(), 1);
        assert_eq!(pending.uids.len(), MOST_PENDING);
        assert_eq!(pending.dropped, 1);
        assert_eq!(pending.batches().len(), MOST_PENDING / UIDS_PER_CHECK);
    }

    #[test]
    fn a_check_ends_once_nothing_is_pending_and_the_next_uid_starts_another() {
        let mut pending = Pending::default();
        assert!(pending.add(1));

        assert!(pending.keeps_checking());
        pending.uids.clear();
        assert!(!pending.keeps_checking());

        assert!(pending.add(2));
    }

    #[test]
    fn a_key_that_cannot_read_tasks_stops_the_watch() {
        let mut pending = Pending {
            unreadable: true,
            ..Pending::default()
        };

        assert!(!pending.add(1));
        assert!(pending.uids.is_empty());
    }
}
