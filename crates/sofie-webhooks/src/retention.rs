use std::time::Duration;

use sofie::prelude::*;

use crate::WebhookDelivery;

pub(crate) const JOB: &str = "sofie_webhooks.prune";
pub(crate) const EVERY: Duration = Duration::from_hours(1);
/// Long enough for an owner to look into a failure and resend it.
const KEPT: Duration = Duration::from_hours(24 * 30);

pub(crate) async fn prune(prune: Prune, clock: Clock) -> Result<()> {
    let before = clock.now().checked_sub(KEPT)?;
    prune
        .before(WebhookDelivery::CREATED_AT, before)
        .await
        .map(drop)
}
