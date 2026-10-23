use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Duration;

use sofie_core::{Timestamp, Uuid, unpoisoned};

const SWEEP_FLOOR: usize = 1024;
/// A few megabytes: past it the requesters closest to refilled are forgotten, so a flood can
/// neither exhaust memory nor lock newcomers out.
const MAX_REQUESTERS: usize = 65_536;
const EVICTED_WHEN_FULL: usize = MAX_REQUESTERS / 16;

/// A token bucket: all at once, then one more each `period / count`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    count: u32,
    period: Duration,
}

impl Rate {
    #[must_use]
    pub const fn per_second(count: u32) -> Self {
        Self::new(count, Duration::from_secs(1))
    }

    #[must_use]
    pub const fn per_minute(count: u32) -> Self {
        Self::new(count, Duration::from_secs(60))
    }

    #[must_use]
    pub const fn per_hour(count: u32) -> Self {
        Self::new(count, Duration::from_hours(1))
    }

    const fn new(count: u32, period: Duration) -> Self {
        assert!(count > 0, "a rate allows at least one request");
        Self { count, period }
    }

    fn interval_micros(self) -> i64 {
        let period = i64::try_from(self.period.as_micros()).unwrap_or(i64::MAX);
        (period / i64::from(self.count)).max(1)
    }
}

/// An IPv6 client is counted by its /64, which one subscriber usually holds whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Requester {
    Principal(Uuid),
    Address(IpAddr),
}

impl Requester {
    pub(crate) fn of(principal: Option<Uuid>, address: IpAddr) -> Self {
        if let Some(principal) = principal {
            return Self::Principal(principal);
        }
        match address.to_canonical() {
            IpAddr::V4(v4) => Self::Address(v4.into()),
            IpAddr::V6(v6) => {
                Self::Address(IpAddr::V6((v6.to_bits() & !u128::from(u64::MAX)).into()))
            }
        }
    }
}

/// Per process: behind a load balancer the effective limit is the rate times the instance count.
pub(crate) struct Limiter {
    rate: Rate,
    requesters: Mutex<Requesters>,
}

/// The generic cell rate algorithm's one number per caller, in microseconds.
struct Requesters {
    due: HashMap<Requester, i64>,
    sweep_at: usize,
}

impl Limiter {
    pub(crate) fn new(rate: Rate) -> Self {
        Self {
            rate,
            requesters: Mutex::new(Requesters {
                due: HashMap::new(),
                sweep_at: SWEEP_FLOOR,
            }),
        }
    }

    pub(crate) fn admit(&self, requester: Requester, now: Timestamp) -> Result<(), Duration> {
        let now = now.as_microsecond();
        let mut requesters = unpoisoned::lock(&self.requesters);
        requesters.make_room_for(requester, now);
        let due = self.due(&requesters, requester, now)?;
        requesters
            .due
            .insert(requester, due + self.rate.interval_micros());
        requesters.sweep(now);
        Ok(())
    }

    /// As `admit`, without counting this request.
    #[cfg(any(test, feature = "tokens"))]
    pub(crate) fn check(&self, requester: Requester, now: Timestamp) -> Result<(), Duration> {
        let requesters = unpoisoned::lock(&self.requesters);
        self.due(&requesters, requester, now.as_microsecond())
            .map(drop)
    }

    fn due(
        &self,
        requesters: &Requesters,
        requester: Requester,
        now: i64,
    ) -> Result<i64, Duration> {
        let tolerance = self.rate.interval_micros() * i64::from(self.rate.count - 1);
        let due = requesters
            .due
            .get(&requester)
            .map_or(now, |due| (*due).max(now));
        let early = due - now;
        if early > tolerance {
            let wait = u64::try_from(early - tolerance).unwrap_or(u64::MAX);
            return Err(Duration::from_micros(wait));
        }
        Ok(due)
    }
}

impl Requesters {
    fn make_room_for(&mut self, requester: Requester, now: i64) {
        if self.due.len() < MAX_REQUESTERS || self.due.contains_key(&requester) {
            return;
        }
        self.due.retain(|_, due| *due > now);
        if self.due.len() < MAX_REQUESTERS {
            return;
        }
        let mut dues: Vec<i64> = self.due.values().copied().collect();
        let (_, cutoff, _) = dues.select_nth_unstable(EVICTED_WHEN_FULL);
        let cutoff = *cutoff;
        self.due.retain(|_, due| *due > cutoff);
    }

    /// Amortized: sweeps only once the map doubled since the last sweep.
    fn sweep(&mut self, now: i64) {
        if self.due.len() < self.sweep_at {
            return;
        }
        self.due.retain(|_, due| *due > now);
        self.sweep_at = (self.due.len() * 2).max(SWEEP_FLOOR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(micros: i64) -> Timestamp {
        Timestamp::from_microsecond(1_700_000_000_000_000 + micros).unwrap()
    }

    fn requester(last: u8) -> Requester {
        Requester::of(None, IpAddr::from([192, 0, 2, last]))
    }

    #[test]
    fn a_burst_of_the_whole_rate_passes_then_one_per_interval() {
        let limiter = Limiter::new(Rate::per_second(4));

        for _ in 0..4 {
            assert!(limiter.admit(requester(1), at(0)).is_ok());
        }
        assert_eq!(
            limiter.admit(requester(1), at(0)),
            Err(Duration::from_millis(250))
        );
        assert!(
            limiter.admit(requester(2), at(0)).is_ok(),
            "requesters count apart"
        );
        assert!(limiter.admit(requester(1), at(250_000)).is_ok());
        assert!(limiter.admit(requester(1), at(250_000)).is_err());
    }

    #[test]
    fn a_check_tells_a_spent_budget_without_spending_any() {
        let limiter = Limiter::new(Rate::per_second(2));

        assert!(limiter.check(requester(1), at(0)).is_ok());
        assert!(limiter.check(requester(1), at(0)).is_ok());
        limiter.admit(requester(1), at(0)).unwrap();
        limiter.admit(requester(1), at(0)).unwrap();

        assert_eq!(
            limiter.check(requester(1), at(0)),
            Err(Duration::from_millis(500))
        );
    }

    #[test]
    fn a_principal_is_counted_over_its_address_and_ipv6_by_its_64() {
        let principal = Uuid::now_v7().unwrap();
        let first: IpAddr = "2001:db8::1".parse().unwrap();
        let second: IpAddr = "2001:db8::ffff:2".parse().unwrap();

        assert_eq!(Requester::of(None, first), Requester::of(None, second));
        assert_eq!(
            Requester::of(Some(principal), first),
            Requester::Principal(principal)
        );
    }

    #[test]
    fn an_ipv4_mapped_address_is_counted_as_its_ipv4_address() {
        let mapped = |text: &str| Requester::of(None, text.parse().unwrap());

        assert_eq!(mapped("::ffff:192.0.2.1"), mapped("192.0.2.1"));
        assert_ne!(mapped("::ffff:192.0.2.1"), mapped("::ffff:198.51.100.1"));
    }

    #[test]
    fn requesters_whose_budget_refilled_are_forgotten() {
        let limiter = Limiter::new(Rate::per_second(1));
        let wave = |first: u8, now| {
            for third in 0..4 {
                for last in 0..=u8::MAX {
                    let address = IpAddr::from([10, first, third, last]);
                    limiter.admit(Requester::of(None, address), now).unwrap();
                }
            }
        };

        wave(0, at(0));
        wave(1, at(2_000_000));

        let requesters = limiter.requesters.lock().unwrap();
        assert_eq!(requesters.due.len(), 1024);
    }

    #[test]
    fn a_full_table_forgets_the_requesters_closest_to_refilled() {
        let limiter = Limiter::new(Rate::per_second(1));
        let address = |index: u32| Requester::of(None, IpAddr::from(index.to_be_bytes()));
        let table_size = u32::try_from(MAX_REQUESTERS).unwrap();
        {
            let mut requesters = limiter.requesters.lock().unwrap();
            for index in 0..table_size {
                let due = at(2_000_000 + i64::from(index)).as_microsecond();
                requesters.due.insert(address(index), due);
            }
            requesters.sweep_at = usize::MAX;
        }

        assert!(limiter.admit(address(u32::MAX), at(0)).is_ok());
        let requesters = limiter.requesters.lock().unwrap();
        assert!(requesters.due.len() < MAX_REQUESTERS);
        assert!(!requesters.due.contains_key(&address(0)));
        assert!(
            requesters.due.contains_key(&address(table_size - 1)),
            "the requester furthest from refilled is still counted"
        );
    }
}
