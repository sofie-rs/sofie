use std::sync::{Arc, OnceLock};

use sofie::core::{Error, Result};
use sofie::db::BoxFuture;
use sofie::db::worker::JobContext;
use sofie::db::{Context, Resolve};
use sofie_client::{Outgoing, Vendor};

const VENDOR: &str = "webhooks";

/// By default public HTTPS hosts only, as owners choose the URLs (SSRF); a test provides its own
/// with `sofie::db::provide::<dyn WebhookSender>`.
pub trait WebhookSender: Send + Sync {
    /// The answer's status; `Error::BadRequest` when the URL was refused before sending.
    fn post<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(&'static str, String)],
        body: &'a str,
    ) -> BoxFuture<'a, Result<u16>>;
}

struct Untrusted(Outgoing);

impl WebhookSender for Untrusted {
    fn post<'a>(
        &'a self,
        url: &'a str,
        headers: &'a [(&'static str, String)],
        body: &'a str,
    ) -> BoxFuture<'a, Result<u16>> {
        Box::pin(async move {
            let untrusted = self.0.untrusted();
            let request = headers
                .iter()
                .fold(untrusted.post(url), |request, (name, value)| {
                    request.header(name, value)
                });
            let answer = request
                .body("application/json", body.to_owned())
                .send()
                .await?;
            Ok(answer.status())
        })
    }
}

pub(crate) struct Transport(pub(crate) Arc<dyn WebhookSender>);

impl Resolve<JobContext> for Transport {
    fn resolve(context: &mut JobContext) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(transport(context))
    }
}

/// One client for the process, so its connections are reused across deliveries.
static UNTRUSTED: OnceLock<Arc<dyn WebhookSender>> = OnceLock::new();

fn transport(context: &JobContext) -> Result<Transport> {
    if let Some(provided) = sofie::db::provided::<dyn WebhookSender>(context.db()) {
        return Ok(Transport(provided));
    }
    if let Some(built) = UNTRUSTED.get() {
        return Ok(Transport(Arc::clone(built)));
    }
    let outgoing = sofie_client::for_vendor(VENDOR, Vendor::default())
        .map_err(|error| Error::internal("building the webhooks' HTTP client", error))?;
    let built = UNTRUSTED.get_or_init(|| Arc::new(Untrusted(outgoing)));
    Ok(Transport(Arc::clone(built)))
}
