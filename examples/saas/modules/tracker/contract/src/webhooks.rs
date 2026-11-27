use sofie_webhooks::{WebhookArea, WebhookEvent};

use crate::area::Team;

pub const TASK_COMPLETED: WebhookEvent =
    WebhookEvent::new("task.completed", "A task of the team was completed.");

impl WebhookArea for Team {
    const WEBHOOK_EVENTS: &'static [WebhookEvent] = &[TASK_COMPLETED];
}
