use sofie::prelude::*;

use crate::note::Note;

/// The editor island's props.
#[derive(ReadModel, Default)]
#[read(from = Note)]
pub struct NoteDraft {
    title: String,
    body: String,
}

pub async fn latest_note(read: Read) -> Result<NoteDraft> {
    let latest = NoteDraft::query().latest().first(&read).await?;
    Ok(latest.unwrap_or_default())
}

/// Shown on a page of catalog components.
pub async fn summary(read: Read) -> Result<NoteDraft> {
    latest_note(read).await
}
