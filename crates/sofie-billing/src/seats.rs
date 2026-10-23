use sofie_core::Result;
use sofie_db::IdempotencyKey;

use crate::events::SeatsAsked;
use crate::provider::SeatsChange;
use crate::{Billing, Subscriber};

/// Retried on failure with the same key, so the provider applies the change once. Two jobs in
/// flight may land at the provider out of order, so a job finding a later count asked once its
/// own landed asks for that count again, which a job of its own sends.
pub(crate) async fn change_at_provider(
    asked: SeatsAsked,
    billing: Billing,
    key: IdempotencyKey,
) -> Result<()> {
    let subscriber = Subscriber::new(asked.subscriber);
    let subscription = billing.subscription(subscriber).await?;
    let Some(reference) = subscription
        .as_ref()
        .and_then(|subscription| subscription.asked_of_provider(asked.seats))
    else {
        tracing::debug!(subscriber = %asked.subscriber, "a later seat count replaced this one");
        return Ok(());
    };
    let change = SeatsChange {
        subscription: reference,
        seats: asked.seats,
        key: &key,
    };
    billing
        .catalog
        .setup
        .provider()?
        .change_seats(&change)
        .await?;
    ask_again_if_replaced(&billing, subscriber, asked.seats).await
}

async fn ask_again_if_replaced(billing: &Billing, subscriber: Subscriber, sent: u32) -> Result<()> {
    let Some(mut subscription) = billing.subscription(subscriber).await? else {
        return Ok(());
    };
    let Some(latest) = subscription.seats_asked().filter(|latest| *latest != sent) else {
        return Ok(());
    };
    subscription.ask_seats(latest, billing.now())?;
    billing.db.commit(&mut subscription).await
}
