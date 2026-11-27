use sofie::billing::Entitlements;
use sofie::prelude::*;

use crate::plans::{OPEN_TASKS, PLANS};
use crate::task::{Task, TaskStatus};

#[derive(ReadModel)]
pub struct StaffHome {}

pub async fn staff_home(_read: Read) -> Result<StaffHome> {
    Ok(StaffHome {})
}

#[derive(ReadModel)]
#[read(from = Task)]
pub struct TaskRow {
    id: Id<Task>,
    title: String,
    status: TaskStatus,
}

pub async fn list_tasks(read: Read) -> Result<Vec<TaskRow>> {
    TaskRow::query().latest().limit(100).all(&read).await
}

pub(crate) async fn open_task_count(read: &Read) -> Result<u64> {
    TaskRow::query()
        .where_(Task::STATUS.eq(TaskStatus::Open))
        .count(read)
        .await
}

#[derive(ReadModel)]
pub struct PlanOffer {
    name: String,
    open_tasks: String,
    trial_days: u64,
}

pub async fn list_plans(_read: Read) -> Result<Vec<PlanOffer>> {
    Ok(PLANS
        .all()
        .map(|plan| PlanOffer {
            name: plan.name().to_owned(),
            open_tasks: shown_limit(plan.limit(OPEN_TASKS)),
            trial_days: u64::from(plan.trial()),
        })
        .collect())
}

#[derive(ReadModel)]
pub struct BillingOverview {
    plan: String,
    status: Option<String>,
    open_tasks: u64,
    open_task_limit: String,
}

pub async fn billing_overview(read: Read, entitlements: Entitlements) -> Result<BillingOverview> {
    let plan = entitlements.plan();
    Ok(BillingOverview {
        plan: plan.name().to_owned(),
        status: entitlements
            .standing()
            .map(|standing| format!("{:?}", standing.status())),
        open_tasks: open_task_count(&read).await?,
        open_task_limit: shown_limit(plan.limit(OPEN_TASKS)),
    })
}

fn shown_limit(limit: u64) -> String {
    if limit == sofie::billing::UNLIMITED {
        return "unlimited".to_owned();
    }
    limit.to_string()
}
