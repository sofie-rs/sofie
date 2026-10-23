use sofie_db::{Call, Context, Jobs, Resolve};
use std::sync::Arc;

use http::Method;
use serde_json::Value;
use sofie_core::{Error, Result};
use sofie_db::worker::JobContext;

use crate::handler::{Endpoint, committed, instrumented};
use crate::routes::{ActionRoute, RouteRules, View};
use crate::trace::handler_name;
use crate::{Area, RequestContext, Routes};

impl<A: Area> Routes<A> {
    /// Queues `job` with the input and path parameters, which bind its records as an action's.
    /// It runs as the requester but reaches every record, so an area whose principals own their
    /// records cannot queue.
    pub fn queue<Args: 'static, J: Call<JobContext, Args>>(
        &mut self,
        path: &str,
        job: J,
    ) -> ActionRoute<'_, A> {
        let endpoint: Endpoint = Arc::new(move |db, matched| {
            let job = job.clone();
            instrumented(handler_name::<J>(), matched.route, async move {
                let mut context = RequestContext::<A>::new(db, matched);
                let queued = queued(&mut context, job).await?;
                committed(context.db(), &context.matched, Box::new(queued)).await
            })
        });
        let rules = RouteRules {
            queues: true,
            ..RouteRules::default()
        };
        ActionRoute::new(self.push(Method::POST, path, endpoint, View::Json, rules))
    }
}

async fn queued<A: Area, Args, J: Call<JobContext, Args>>(
    context: &mut RequestContext<A>,
    job: J,
) -> Result<sofie_db::Queued> {
    let Value::Object(mut payload) = context.input()? else {
        return Err(Error::BadRequest(
            "a queued job takes an object of fields".to_owned(),
        ));
    };
    for (name, value) in &context.matched.parameters {
        payload.insert(name.clone(), Value::String(value.clone()));
    }
    Jobs::resolve(context).await?.queue(job, &payload)
}
