use sofie::prelude::*;
use sofie_tenancy::OrganizationId;

use super::{Task, TaskStatus};

pub fn open(organization: OrganizationId, title: String) -> Result<Task> {
    Ok(Task {
        meta: Meta::new()?,
        organization,
        title,
        status: TaskStatus::Open,
    })
}
