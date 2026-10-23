use std::time::Duration;

use sofie_core::{Clock, Result};
use sofie_db::Prune;

use crate::store::{DIGEST_SENT_AT, INBOX_READ_AT};

pub(crate) const JOB: &str = "sofie_notify.prune";
pub(crate) const EVERY: Duration = Duration::from_hours(1);
const KEPT: Duration = Duration::from_hours(24 * 90);

/// Unread inbox items and digest items not yet mailed stay, however old.
pub(crate) async fn prune(prune: Prune, clock: Clock) -> Result<()> {
    let before = Some(clock.now().checked_sub(KEPT)?.timestamp());
    prune.before(INBOX_READ_AT, before).await?;
    prune.before(DIGEST_SENT_AT, before).await.map(drop)
}
