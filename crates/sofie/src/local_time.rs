//! The `#[every]` attribute and `sofie gen` wrap what a wall-clock schedule needs in
//! `__needs_local_time!`, so an app without the feature is told which one to enable rather than
//! that `WallClock` is missing.

/// ```
/// #[sofie::every(daily_at = "03:00", zone = "Europe/Istanbul")]
/// pub async fn digest() {}
/// ```
#[cfg(feature = "local-time")]
#[doc(hidden)]
#[macro_export]
macro_rules! __needs_local_time {
    ($($wall_clock:tt)*) => { $($wall_clock)* };
}

/// ```compile_fail
/// #[sofie::every(daily_at = "03:00", zone = "Europe/Istanbul")]
/// pub async fn digest() {}
/// ```
#[cfg(not(feature = "local-time"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __needs_local_time {
    ($($wall_clock:tt)*) => {
        ::core::compile_error!(
            "a `daily_at` schedule needs sofie's `local-time` feature: add `features = [\"local-time\"]` to the app's sofie dependency"
        )
    };
}
