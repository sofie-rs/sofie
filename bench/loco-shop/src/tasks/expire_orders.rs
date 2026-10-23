use loco_rs::prelude::*;

use crate::models::orders;

const PAYMENT_WINDOW_HOURS: i64 = 24;

pub struct ExpireOrders;

#[async_trait]
impl Task for ExpireOrders {
    fn task(&self) -> TaskInfo {
        TaskInfo {
            name: "expire_orders".to_string(),
            detail: "Expire orders still pending after the payment window".to_string(),
        }
    }

    async fn run(&self, ctx: &AppContext, _vars: &task::Vars) -> Result<()> {
        let placed_before = chrono::Utc::now() - chrono::Duration::hours(PAYMENT_WINDOW_HOURS);
        let expired = orders::Model::expire_pending(&ctx.db, placed_before.into()).await?;
        tracing::info!(expired, "expired unpaid orders");
        Ok(())
    }
}
