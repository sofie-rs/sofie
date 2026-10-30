use std::sync::Arc;

use sofie_core::Result;
use sofie_db::{AnyOutcome, BoxFuture, Call, Context, Db, ReadOnly, Reply, Run, erase};
use tracing::Instrument;

use crate::context::Matched;
use crate::trace::{HandlerName, handler_name, handler_span};
use crate::{Area, ReadContext, RequestContext};

/// Area erased, so routes of every area share one router.
pub(crate) type Endpoint =
    Arc<dyn Fn(Db, Matched) -> BoxFuture<'static, Result<Reply>> + Send + Sync>;

/// What a GET, a screen or an island is answered by.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot answer a GET, a screen or an island: a GET never writes",
    label = "takes a write-side argument or returns a writable outcome",
    note = "take only read-side arguments (`Read`, a read model, `Principal`, `Features`, `Permissions`, `Listing`, config, the clock) and return a read model, `Paginated`, `Screen` or another reply; a write belongs in a POST, or in a job a domain event triggers"
)]
pub trait Loader<A: Area, Args>: Call<ReadContext<A>, Args, Outcome: ReadOnly> {}

impl<A: Area, Args, H: Call<ReadContext<A>, Args, Outcome: ReadOnly>> Loader<A, Args> for H {}

pub(crate) fn endpoint<A: Area, Args, F: Call<RequestContext<A>, Args>>(handler: F) -> Endpoint {
    writing(erase(handler), handler_name::<F>())
}

fn writing<A: Area>(run: Run<RequestContext<A>>, name: HandlerName) -> Endpoint {
    Arc::new(move |db, matched| {
        let run = Arc::clone(&run);
        instrumented(name, matched.route, async move {
            let mut context = RequestContext::<A>::new(db, matched);
            #[cfg(feature = "idempotency")]
            if let Some(work) = context.matched.work.clone() {
                return crate::keyed::answer_once(&run, context, &work).await;
            }
            let outcome = run(&mut context).await?;
            committed(context.db(), &context.matched, outcome).await
        })
    })
}

/// Lets the session write what the handler asked only once its outcome is committed.
pub(crate) async fn committed(
    db: &Db,
    matched: &Matched,
    mut outcome: Box<dyn AnyOutcome>,
) -> Result<Reply> {
    matched.deadline.lift();
    db.commit_with(&mut *outcome, Vec::new(), matched.reach)
        .await?;
    matched.session.committed();
    #[cfg(feature = "uploads")]
    crate::uploads::committed();
    outcome.into_reply()
}

pub(crate) fn loader_endpoint<A: Area, Args, F: Loader<A, Args>>(loader: F) -> Endpoint {
    let run = erase::<ReadContext<A>, _, _>(loader);
    let name = handler_name::<F>();
    Arc::new(move |db, matched| {
        let run = Arc::clone(&run);
        instrumented(name, matched.route, async move {
            let mut context = ReadContext::<A>::new(db, matched);
            run(&mut context).await?.into_reply()
        })
    })
}

/// History records the route, or else the handler's path, for every commit the handler makes.
pub(crate) fn instrumented(
    name: HandlerName,
    route: Option<&'static str>,
    work: impl Future<Output = Result<Reply>> + Send + 'static,
) -> BoxFuture<'static, Result<Reply>> {
    let action = route.unwrap_or(name.0);
    let work = sofie_db::action::acting_for(action, work).instrument(handler_span(name));
    #[cfg(feature = "metrics")]
    let work = crate::metrics::timed(name, work);
    Box::pin(work)
}
