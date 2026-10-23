use std::sync::Arc;

use sofie_core::Result;

use crate::{BoxFuture, Outcome, Reply, Resolve};

pub trait Call<C, Args>: Clone + Send + Sync + 'static {
    type Outcome: Outcome + 'static;

    fn call(&self, context: &mut C) -> impl Future<Output = Result<Self::Outcome>> + Send;
}

macro_rules! call {
    ($($argument:ident),*) => {
        impl<C, F, Fut, O, $($argument,)*> Call<C, ($($argument,)*)> for F
        where
            C: Send,
            F: Fn($($argument),*) -> Fut + Clone + Send + Sync + 'static,
            Fut: Future<Output = Result<O>> + Send,
            O: Outcome + 'static,
            $($argument: Resolve<C> + Send,)*
        {
            type Outcome = O;

            #[allow(non_snake_case, unused_variables, reason = "arguments are named after their types")]
            fn call(&self, context: &mut C) -> impl Future<Output = Result<O>> + Send {
                async move {
                    $(let $argument = $argument::resolve(context).await?;)*
                    self($($argument),*).await
                }
            }
        }
    };
}

macro_rules! calls {
    () => {
        call!();
    };
    ($first:ident $(, $rest:ident)*) => {
        call!($first $(, $rest)*);
        calls!($($rest),*);
    };
}

calls!(T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12);

pub trait AnyOutcome: Outcome {
    fn into_reply(self: Box<Self>) -> Result<Reply>;
}

impl<O: Outcome> AnyOutcome for O {
    fn into_reply(self: Box<Self>) -> Result<Reply> {
        (*self).reply()
    }
}

pub type Run<C> =
    Arc<dyn for<'c> Fn(&'c mut C) -> BoxFuture<'c, Result<Box<dyn AnyOutcome>>> + Send + Sync>;

pub fn erase<C: Send + 'static, Args, F: Call<C, Args>>(function: F) -> Run<C> {
    Arc::new(move |context| {
        let function = function.clone();
        Box::pin(async move {
            let outcome = function.call(context).await?;
            Ok(Box::new(outcome) as Box<dyn AnyOutcome>)
        })
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// A message naming it: a queued job, or each record's job of a schedule.
    Queued,
    On(&'static str),
    Every(crate::worker::Cadence),
}

pub struct Entry<C> {
    pub name: &'static str,
    pub trigger: Trigger,
    pub run: Run<C>,
}
