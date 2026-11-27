use sofie::billing::{Billing, Checkout, Entitlements, Link, TrialStarted};
use sofie::prelude::*;
use tracker_contract::area::Team;

use crate::plans::{OPEN_TASKS, PRO};
use crate::queries;
use crate::task::{Task, factory};

#[derive(Input)]
pub struct NewTask {
    title: String,
}

pub async fn create_task(
    team: Principal<Team>,
    input: NewTask,
    read: Read,
    entitlements: Entitlements,
) -> Result<Task> {
    entitlements.allow_another(OPEN_TASKS, queries::open_task_count(&read).await?)?;
    factory::open(team.id(), input.title)
}

pub async fn complete_task(mut task: Task) -> Result<Task> {
    task.complete()?;
    Ok(task)
}

pub async fn reopen_task(mut task: Task) -> Result<Task> {
    task.reopen()?;
    Ok(task)
}

pub async fn start_trial(team: Principal<Team>, billing: Billing) -> Result<TrialStarted> {
    billing.start_trial(team.id().into(), PRO).await
}

pub async fn upgrade(team: Principal<Team>, checkout: Checkout) -> Result<Link> {
    checkout
        .subscribe(team.id().into(), PRO, 1, BILLING_PAGE)
        .await
}

pub async fn manage_billing(team: Principal<Team>, checkout: Checkout) -> Result<Link> {
    checkout.portal(team.id().into(), BILLING_PAGE).await
}

const BILLING_PAGE: &str = "/team/billing";
