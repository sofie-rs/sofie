use std::sync::atomic::{AtomicBool, Ordering};

static FROM_TESTS: AtomicBool = AtomicBool::new(false);

/// sofie-testing calls it before it builds an app, whose tests then give each gateway and split
/// module a fake: the wiring loads no adapter and connects nowhere. A feature could not say so,
/// as a workspace build would turn it on in the production binary too.
#[doc(hidden)]
pub fn fakes_from_tests() {
    FROM_TESTS.store(true, Ordering::Relaxed);
}

#[cfg(feature = "client")]
pub(crate) fn provided_by_tests() -> bool {
    FROM_TESTS.load(Ordering::Relaxed)
}
