use std::cell::RefCell;

use tracing::span::Id;

thread_local! {
    /// Tagged with the owning store: a thread can switch subscribers while inside a span.
    static ENTERED: RefCell<Vec<(u64, u64)>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn push(owner: u64, id: &Id) {
    with_stack(|stack| stack.push((owner, id.into_u64())));
}

/// Removes the innermost entry of `id`: futures may exit spans out of order.
pub(crate) fn pop(owner: u64, id: &Id) {
    let entry = (owner, id.into_u64());
    with_stack(|stack| {
        if let Some(position) = stack.iter().rposition(|entered| *entered == entry) {
            stack.remove(position);
        }
    });
}

pub(crate) fn current(owner: u64) -> Option<Id> {
    with_stack(|stack| {
        stack
            .iter()
            .rev()
            .find(|(entered_owner, _)| *entered_owner == owner)
            .map(|(_, id)| Id::from_u64(*id))
    })
    .flatten()
}

/// A thread being torn down has dropped its stack and is inside no span.
fn with_stack<T>(visit: impl FnOnce(&mut Vec<(u64, u64)>) -> T) -> Option<T> {
    ENTERED
        .try_with(|stack| visit(&mut stack.borrow_mut()))
        .ok()
}
