use serde_json::{Value, json};
use sofie_core::Result;
use sofie_db::{Aggregate, Read, history_of};

/// Empty for an aggregate that does not record its changes.
pub(super) async fn changes<R: Aggregate>(read: &Read, record: &R) -> Result<Value> {
    let changes = history_of::<R>(read, record.id()).await?;
    Ok(changes
        .into_iter()
        .map(|change| {
            json!({
                "change": format!("{:?}", change.kind),
                "fields": change.fields.join(", "),
                "actor": change.actor.map(|actor| actor.to_string()),
                "action": change.action,
                "at": change.at,
            })
        })
        .collect())
}
