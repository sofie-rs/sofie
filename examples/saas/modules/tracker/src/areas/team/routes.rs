use sofie::prelude::*;
use tracker_contract::area::Team;

use crate::module::REOPENING;
use crate::{actions, queries};

pub fn routes(routes: &mut Routes<Team>) {
    routes
        .screen("/tasks", "tasks", queries::list_tasks)
        .live("tasks");
    routes
        .post("/tasks", actions::create_task)
        .then_screen("tasks", "tasks");
    routes
        .post("/tasks/{task}/complete", actions::complete_task)
        .then_screen("tasks", "tasks");
    routes
        .post("/tasks/{task}/reopen", actions::reopen_task)
        .feature(REOPENING)
        .then_screen("tasks", "tasks");
    routes.screen("/billing", "billing", queries::billing_overview);
    routes
        .post("/billing/trial", actions::start_trial)
        .then_screen("billing", "billing");
    routes
        .post("/billing/upgrade", actions::upgrade)
        .then_follow();
    routes
        .post("/billing/portal", actions::manage_billing)
        .then_follow();
}
