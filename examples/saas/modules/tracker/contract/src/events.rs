use sofie::db::Uuid;
use sofie::prelude::*;

#[derive(Event)]
pub struct TaskCompleted {
    pub task: Uuid,
    pub organization: Uuid,
    pub title: String,
}
