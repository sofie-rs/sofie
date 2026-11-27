use sofie::db::Queued;
use sofie::prelude::*;
use sofie::serde_json::json;
use sofie_webhooks::Webhooks;
use tracker_contract::{TASK_COMPLETED, TaskCompleted};

#[on]
pub async fn announce_completion(event: TaskCompleted, webhooks: Webhooks) -> Result<Queued> {
    webhooks.send(
        event.organization,
        &TASK_COMPLETED,
        &json!({ "task": event.task, "title": event.title }),
    )
}
