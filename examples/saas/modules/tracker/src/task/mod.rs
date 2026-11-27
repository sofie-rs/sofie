pub mod factory;

use sofie::prelude::*;
use sofie_tenancy::OrganizationId;

use tracker_contract::TaskCompleted;

#[derive(Aggregate)]
pub struct Task {
    meta: Meta,
    #[owner]
    organization: OrganizationId,
    #[length(min = 1, max = 120)]
    title: String,
    status: TaskStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Enum)]
pub enum TaskStatus {
    #[to(Done)]
    Open,
    #[to(Open)]
    Done,
}

impl Task {
    pub fn complete(&mut self) -> Result<()> {
        self.status = self.status.to(TaskStatus::Done, "status")?;
        self.meta.raise(TaskCompleted {
            task: self.id().uuid(),
            organization: self.organization.uuid(),
            title: self.title.clone(),
        })
    }

    pub fn reopen(&mut self) -> Result<()> {
        self.status = self.status.to(TaskStatus::Open, "status")?;
        Ok(())
    }
}
