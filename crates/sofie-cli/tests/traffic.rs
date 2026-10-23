use std::path::{Path, PathBuf};

use sofie_cli::{FuzzOptions, fuzz, seed};

fn shop() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/shop")
}

#[test]
#[ignore = "builds and runs the shop: needs DATABASE_URL and SOFIE_SIGNING_KEYS of a real Postgres"]
fn fuzzing_the_shop_reaches_its_handlers_and_finds_no_500() {
    let report = fuzz(
        &shop(),
        FuzzOptions::new(Some(1), std::num::NonZeroUsize::new(80)),
    )
    .unwrap();

    assert_eq!((report.sent, report.routes), (80, 5), "{report}");
    let answered = |range: std::ops::Range<u16>| {
        report
            .statuses
            .iter()
            .any(|(status, _)| range.contains(status))
    };
    assert!(answered(200..300) && answered(400..500), "{report}");
    assert_eq!(report.findings, Vec::new(), "{report}");
}

#[test]
#[ignore = "builds and runs the shop: needs DATABASE_URL and SOFIE_SIGNING_KEYS of a real Postgres"]
fn seeding_the_shop_posts_its_seed_file_and_the_volume_to_each_creating_route() {
    let report = seed(&shop(), 5).unwrap();

    let routes: Vec<(&str, usize, usize)> = report
        .routes
        .iter()
        .map(|route| (route.route.as_str(), route.created, route.refused))
        .collect();
    assert_eq!(routes, [("order.customer.place_order", 7, 0)], "{report}");
}
