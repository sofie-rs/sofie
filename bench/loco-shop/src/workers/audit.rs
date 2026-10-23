use loco_rs::prelude::*;
use serde::{Deserialize, Serialize};

use crate::models::audit_entries;

pub struct Worker {
    pub ctx: AppContext,
}

#[derive(Deserialize, Debug, Serialize)]
pub struct WorkerArgs {
    pub order_id: i64,
    pub action: String,
}

#[async_trait]
impl BackgroundWorker<WorkerArgs> for Worker {
    fn build(ctx: &AppContext) -> Self {
        Self { ctx: ctx.clone() }
    }

    fn class_name() -> String {
        "Audit".to_string()
    }

    async fn perform(&self, args: WorkerArgs) -> Result<()> {
        audit_entries::Model::record(&self.ctx.db, args.order_id, &args.action).await?;
        Ok(())
    }
}
