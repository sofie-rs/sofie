use sofie_core::DisplayZone;

/// Called first by every test in the binary: reading an instant fixes the zone for the whole
/// binary, as for the app's process.
pub fn set_display_zone(name: &str) {
    let zone = DisplayZone::named(name).unwrap_or_else(|error| panic!("{error}"));
    if let Err(error) = zone.install() {
        panic!(
            "{error}: a test binary has one display zone, so a test in another zone goes in its own file under tests/"
        );
    }
}
