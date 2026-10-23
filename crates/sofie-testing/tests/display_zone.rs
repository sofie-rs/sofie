use std::panic;

use sofie::prelude::Instant;

#[test]
fn a_test_binary_reads_wall_clocks_in_the_one_zone_it_sets() {
    sofie_testing::set_display_zone("Europe/Istanbul");
    sofie_testing::set_display_zone("Europe/Istanbul");

    assert_eq!(
        Instant::parse("2026-10-11T12:30").unwrap(),
        Instant::parse("2026-10-11T09:30:00Z").unwrap()
    );
    let refused = panic::catch_unwind(|| sofie_testing::set_display_zone("Europe/Berlin"));
    let message = *refused.unwrap_err().downcast::<String>().unwrap();
    assert!(message.contains("one display zone"), "{message}");
    assert!(message.contains("Europe/Istanbul"), "{message}");
}
