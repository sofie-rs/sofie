use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sofie_core::unpoisoned;

use crate::throttle::Requester;

/// Tabs of one person, or callers behind one address (IPv6 by /64).
const PER_HOLDER: usize = 8;
const PER_PROCESS: usize = 10_000;

/// Long-lived connections a process keeps open, capped per holder and in all.
#[derive(Default)]
pub(crate) struct Holders(Arc<Mutex<Open>>);

#[derive(Default)]
struct Open {
    total: usize,
    held: HashMap<Requester, usize>,
}

impl Holders {
    pub(crate) fn admit(&self, holder: Requester) -> Option<Held> {
        let mut open = unpoisoned::lock(&self.0);
        let held = open.held.get(&holder).copied().unwrap_or(0);
        if open.total >= PER_PROCESS || held >= PER_HOLDER {
            return None;
        }
        open.total += 1;
        open.held.insert(holder, held + 1);
        Some(Held {
            open: Arc::clone(&self.0),
            holder,
        })
    }
}

/// Frees its place when its connection ends, however it ends.
pub(crate) struct Held {
    open: Arc<Mutex<Open>>,
    holder: Requester,
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut open = unpoisoned::lock(&self.open);
        open.total -= 1;
        let held = open.held.get_mut(&self.holder).map_or(0, |held| {
            *held -= 1;
            *held
        });
        if held == 0 {
            open.held.remove(&self.holder);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_addresses_of_one_ipv6_64_share_one_holder_s_places() {
        let holders = Holders::default();
        let holder = |last: u16| {
            let address = std::net::Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, last);
            Requester::of(None, std::net::IpAddr::V6(address))
        };

        let held: Vec<Held> = (0..8)
            .filter_map(|last| holders.admit(holder(last)))
            .collect();

        assert_eq!(held.len(), PER_HOLDER);
        assert!(holders.admit(holder(9)).is_none());
    }

    #[test]
    fn a_place_is_freed_when_its_connection_ends() {
        let holders = Holders::default();
        let holder = Requester::of(None, std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        let held: Vec<Held> = (0..PER_HOLDER)
            .filter_map(|_| holders.admit(holder))
            .collect();

        drop(held);

        assert!(holders.admit(holder).is_some());
    }
}
