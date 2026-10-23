use std::sync::OnceLock;

use jiff::civil::DateTime;
use jiff::tz::{AmbiguousOffset, TimeZone};

use crate::{Error, FieldError, Instant, Result};

/// Process-wide because an instant read from a form deserializes with no request at hand.
static INSTALLED: OnceLock<DisplayZone> = OnceLock::new();

/// Where people see and type instants; stored and JSON instants stay UTC.
#[derive(Clone, Debug)]
pub struct DisplayZone {
    zone: TimeZone,
}

impl DisplayZone {
    pub fn named(name: &str) -> Result<Self> {
        let refused = || format!("`{name}` is not an IANA time zone such as Europe/Istanbul");
        let zone = TimeZone::get(name.trim()).map_err(|error| Error::internal(refused(), error))?;
        if !zone.iana_name().is_some_and(is_iana_identifier) {
            return Err(Error::internal_message(refused()));
        }
        Ok(Self { zone })
    }

    /// UTC unless installed first; reading it fixes it, so no instant is read in two zones.
    #[must_use]
    pub fn current() -> &'static Self {
        INSTALLED.get_or_init(|| Self {
            zone: TimeZone::UTC,
        })
    }

    pub fn install(self) -> Result<()> {
        let installed = INSTALLED.get_or_init(|| self.clone());
        if installed.name() == self.name() {
            return Ok(());
        }
        Err(Error::internal_message(format!(
            "the display zone is {}, fixed when first installed or first read, not {}",
            installed.name(),
            self.name()
        )))
    }

    #[must_use]
    pub fn name(&self) -> &str {
        self.zone
            .iana_name()
            .expect("UTC and a zone found in the database carry their name")
    }

    #[must_use]
    pub fn wall_clock(&self, instant: Instant) -> DateTime {
        self.zone.to_datetime(instant.timestamp())
    }

    /// True where a backward shift repeats it, so only an offset tells its two instants apart.
    #[must_use]
    pub fn repeats(&self, wall_clock: DateTime) -> bool {
        matches!(
            self.zone.to_ambiguous_timestamp(wall_clock).offset(),
            AmbiguousOffset::Fold { .. }
        )
    }

    #[must_use]
    pub fn repeated_offset(&self, instant: Instant) -> Option<String> {
        let zoned = instant.timestamp().to_zoned(self.zone.clone());
        self.repeats(zoned.datetime())
            .then(|| zoned.strftime("%:z").to_string())
    }

    /// On the zone's wall clock with its offset there, such as `2026-10-25T02:30:00+01:00`.
    #[must_use]
    pub fn with_offset(&self, instant: Instant) -> String {
        let offset = self.zone.to_offset(instant.timestamp());
        instant.timestamp().display_with_offset(offset).to_string()
    }

    pub fn instant_at(&self, wall_clock: DateTime) -> Result<Instant> {
        let placed = self.zone.to_ambiguous_timestamp(wall_clock);
        if let AmbiguousOffset::Fold { before, after } = placed.offset() {
            let [earlier, later] = [before, after].map(|offset| {
                offset
                    .to_timestamp(wall_clock)
                    .expect("the zone's own offsets place a time it repeats")
                    .display_with_offset(offset)
                    .to_string()
            });
            let refusal = FieldError::new(
                "instant",
                "validation.instant.repeated",
                format!(
                    "{wall_clock} happens twice in {}; give its offset: {earlier} or {later}",
                    self.name()
                ),
            );
            return Err(Error::Invalid(vec![
                refusal
                    .with("wall_clock", wall_clock.to_string())
                    .with("zone", self.name())
                    .with("earlier", earlier)
                    .with("later", later),
            ]));
        }
        wall_clock_instant(&self.zone, wall_clock)
            .map(Instant::from)
            .ok_or_else(|| {
                let refusal = FieldError::new(
                    "instant",
                    "validation.instant.out_of_range",
                    format!("{wall_clock} in {} is out of range", self.name()),
                );
                Error::Invalid(vec![
                    refusal
                        .with("wall_clock", wall_clock.to_string())
                        .with("zone", self.name()),
                ])
            })
    }
}

/// A time a forward shift skips goes where the shift ends, a repeated one to its first occurrence.
#[must_use]
pub fn wall_clock_instant(zone: &TimeZone, local: DateTime) -> Option<jiff::Timestamp> {
    let ambiguous = zone.to_ambiguous_timestamp(local);
    let earlier = ambiguous.earlier().ok()?;
    if !matches!(ambiguous.offset(), AmbiguousOffset::Gap { .. }) {
        return Some(earlier);
    }
    zone.following(earlier)
        .next()
        .map(|shift| shift.timestamp())
}

/// The zoneinfo directory also holds `posixrules`, `Factory`, `+VERSION` and tables, which a
/// browser's `Intl` refuses; tzdb names its zones with components that start with a capital.
fn is_iana_identifier(name: &str) -> bool {
    name != "Factory"
        && name.split('/').all(|component| {
            component.starts_with(|first: char| first.is_ascii_uppercase())
                && component
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || "_-+".contains(character))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_an_instant_on_the_zone_s_wall_clock_and_reads_it_back() {
        let zone = DisplayZone::named("Europe/Istanbul").unwrap();
        let instant = Instant::parse("2026-10-11T09:30:00Z").unwrap();

        let wall_clock = zone.wall_clock(instant);

        assert_eq!(wall_clock.to_string(), "2026-10-11T12:30:00");
        assert_eq!(zone.instant_at(wall_clock).unwrap(), instant);
        assert_eq!(zone.name(), "Europe/Istanbul");
    }

    #[test]
    fn writes_an_instant_in_a_repeated_hour_with_the_offset_that_reads_it_back() {
        let london = DisplayZone::named("Europe/London").unwrap();
        for (at, written) in [
            ("2026-10-25T00:30:00Z", "2026-10-25T01:30:00+01:00"),
            ("2026-10-25T01:30:15.5Z", "2026-10-25T01:30:15.5+00:00"),
        ] {
            let instant = Instant::parse(at).unwrap();

            assert!(london.repeats(london.wall_clock(instant)));
            assert_eq!(london.with_offset(instant), written);
            assert_eq!(Instant::parse_in(written, &london).unwrap(), instant);
        }
        let outside = Instant::parse("2026-10-25T02:30:00Z").unwrap();
        assert!(!london.repeats(london.wall_clock(outside)));
    }

    #[test]
    fn names_an_instant_s_offset_only_where_its_wall_clock_repeats() {
        let berlin = DisplayZone::named("Europe/Berlin").unwrap();
        let offset = |at: &str| berlin.repeated_offset(Instant::parse(at).unwrap());

        assert_eq!(offset("2026-10-25T00:30:00Z").as_deref(), Some("+02:00"));
        assert_eq!(offset("2026-10-25T01:30:00Z").as_deref(), Some("+01:00"));
        assert_eq!(offset("2026-10-25T02:30:00Z"), None);
    }

    #[test]
    fn refuses_an_unknown_zone_naming_it() {
        let error = DisplayZone::named("Mars/Olympus").unwrap_err();
        assert!(error.to_string().contains("`Mars/Olympus`"), "{error}");
    }

    #[test]
    fn refuses_a_zone_a_browser_cannot_show_with_the_same_message() {
        for name in [
            "posix/Europe/Istanbul",
            "right/Europe/Istanbul",
            "Factory",
            "localtime",
            "posixrules",
            "+VERSION",
            "zone.tab",
        ] {
            let error = DisplayZone::named(name).unwrap_err();
            assert!(
                error.to_string().contains("is not an IANA time zone"),
                "{name}: {error}"
            );
        }
    }

    #[test]
    fn refuses_the_unknown_zone_which_has_no_iana_name() {
        let error = DisplayZone::named("Etc/Unknown").unwrap_err();
        assert!(error.to_string().contains("`Etc/Unknown`"), "{error}");
    }

    #[test]
    fn keeps_the_database_s_spelling_of_a_zone() {
        assert_eq!(
            DisplayZone::named("europe/istanbul").unwrap().name(),
            "Europe/Istanbul"
        );
        assert_eq!(DisplayZone::named("utc").unwrap().name(), "UTC");
    }

    #[test]
    fn reading_the_zone_fixes_it_so_a_later_other_zone_is_refused() {
        assert_eq!(DisplayZone::current().name(), "UTC");

        let error = DisplayZone::named("Europe/Berlin")
            .unwrap()
            .install()
            .unwrap_err();

        assert!(error.to_string().contains("first read"), "{error}");
        assert_eq!(DisplayZone::current().name(), "UTC");
        assert!(DisplayZone::named("UTC").unwrap().install().is_ok());
    }

    #[test]
    fn is_utc_until_installed() {
        assert_eq!(
            DisplayZone::current()
                .wall_clock(Instant::parse("2026-10-11T09:30:00Z").unwrap())
                .to_string(),
            "2026-10-11T09:30:00"
        );
    }
}
