use sofie_core::Result;
use sofie_db::{Outcome, Read, ReadOnly, Reply};
use sofie_http::{Query, Routes, Sealing, Shipped, Web};

use super::Suppressed;
use crate::suppression::{self, Reason, Scope};
use crate::unsubscribe::{PATH, PURPOSE, opened};

const SCREEN: &str = r#"<Section id="unsubscribe" title="Unsubscribe">
  {#if data.invalid}<p>This link is not one this app sent, or it was changed.</p>{/if}
  {#if data.done}<p>{data.address} gets no more mail of the list “{data.list}”.</p>{/if}
  {#if data.asking}
    <p>Stop sending {data.address} mail of the list “{data.list}”?</p>
    <form method="post" action={data.action}><button type="submit">Unsubscribe</button></form>
  {/if}
</Section>"#;

pub(super) fn routes(routes: &mut Routes<Web>) {
    routes
        .shipped(PATH, "unsubscribe", (SCREEN, None), page)
        .name("unsubscribe_page");
    // RFC 8058: a mailbox provider posts `List-Unsubscribe=One-Click` here, with no cookie,
    // and takes only a 2xx answer as done.
    routes.post(PATH, unsubscribe).name("unsubscribe");
    routes
        .post(&confirm_path(), unsubscribe)
        .name("confirm_unsubscribe")
        .then_screen("unsubscribe", "unsubscribe");
}

fn confirm_path() -> String {
    format!("{PATH}/confirm")
}

fn read_token(query: &Query, sealing: &Sealing) -> Option<(String, String)> {
    let token = query.get("token")?;
    opened(&sealing.open(PURPOSE, &token)?)
}

pub struct Page(serde_json::Value);

impl Outcome for Page {
    fn reply(self) -> Result<Reply> {
        Ok(Reply::Json(self.0))
    }
}

impl ReadOnly for Page {}

/// Unsubscribes only when posted to, so a link scanner opening the mail changes nothing.
async fn page(query: Query, sealing: Sealing, read: Read) -> Result<Page> {
    let Some((address, list)) = read_token(&query, &sealing) else {
        return Ok(Page(serde_json::json!({ "invalid": true })));
    };
    let scopes = suppression::scopes(&read, &address).await?;
    let done = scopes.iter().any(|scope| scope.blocks(Some(&list)));
    let action = format!(
        "{}?token={}",
        confirm_path(),
        query.get("token").unwrap_or_default()
    );
    Ok(Page(serde_json::json!({
        "address": address,
        "list": list,
        "done": done,
        "asking": !done,
        "action": action,
    })))
}

async fn unsubscribe(query: Query, sealing: Sealing, read: Read) -> Result<Suppressed> {
    let (address, list) = read_token(&query, &sealing).ok_or_else(|| {
        sofie_core::Error::BadRequest("the unsubscribe link is not one this app sent".to_owned())
    })?;
    suppression::suppress(&read, &address, Scope::List(list), Reason::Unsubscribe)
        .await
        .map(Suppressed)
}
