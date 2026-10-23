use std::future::poll_fn;
use std::task::Poll;

use crate::BoxFuture;

/// Polls in order first, so requests sent on a first poll leave as one pipeline.
pub async fn join_all<T>(mut futures: Vec<BoxFuture<'_, T>>) -> Vec<T> {
    let mut outputs: Vec<Option<T>> = futures.iter().map(|_| None).collect();
    poll_fn(|context| {
        let mut pending = false;
        for (future, output) in futures.iter_mut().zip(outputs.iter_mut()) {
            if output.is_some() {
                continue;
            }
            match future.as_mut().poll(context) {
                Poll::Ready(value) => *output = Some(value),
                Poll::Pending => pending = true,
            }
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await;
    outputs.into_iter().flatten().collect()
}
