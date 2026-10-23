#![cfg(all(feature = "person", feature = "time", feature = "web"))]

use sofie::prelude::*;

#[test]
fn the_prelude_names_the_everyday_values() {
    let named: fn(Instant, Date, PersonName, Url, Email, Money) = |_, _, _, _, _, _| {};

    let _ = named;
}
