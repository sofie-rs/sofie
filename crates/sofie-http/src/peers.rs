use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sofie_core::unpoisoned;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::throttle::Requester;

/// A household or an office behind one address stays far below it.
const PER_PEER: usize = 64;
/// Past it a refused connection is dropped unanswered, so refusals cannot hold every permit.
const REFUSING_AT_ONCE: usize = 64;
const WARNING_INTERVAL: Duration = Duration::from_secs(60);

/// Open connections per address (IPv6 by /64), so one client cannot take every connection.
/// Never more entries than open connections, so it needs no bound of its own.
#[derive(Default)]
pub(crate) struct Peers(Arc<Mutex<HashMap<Requester, usize>>>);

/// Frees its place when its connection ends, however it ends.
pub(crate) struct PeerSlot {
    peers: Arc<Mutex<HashMap<Requester, usize>>>,
    peer: Requester,
}

impl Peers {
    /// `None` when the address holds its share already.
    pub(crate) fn admit(&self, address: IpAddr) -> Option<PeerSlot> {
        let peer = Requester::of(None, address);
        let mut held = unpoisoned::lock(&self.0);
        let count = held.entry(peer).or_default();
        if *count >= PER_PEER {
            return None;
        }
        *count += 1;
        Some(PeerSlot {
            peers: Arc::clone(&self.0),
            peer,
        })
    }
}

/// Connections refused for their address's share, answered 503 outside the connection cap.
pub(crate) struct Refusals {
    answering: Arc<Semaphore>,
    unwarned: Mutex<Unwarned>,
}

#[derive(Default)]
struct Unwarned {
    warned_at: Option<Instant>,
    count: u64,
}

impl Default for Refusals {
    fn default() -> Self {
        Self {
            answering: Arc::new(Semaphore::new(REFUSING_AT_ONCE)),
            unwarned: Mutex::default(),
        }
    }
}

impl Refusals {
    /// Frees the connection's permit; `None` when the stream is to be dropped unanswered.
    pub(crate) fn admit(&self, permit: OwnedSemaphorePermit) -> Option<OwnedSemaphorePermit> {
        drop(permit);
        Arc::clone(&self.answering).try_acquire_owned().ok()
    }

    /// The refusals since the last warning, once a minute.
    pub(crate) fn counted(&self, now: Instant) -> Option<u64> {
        let mut unwarned = unpoisoned::lock(&self.unwarned);
        unwarned.count += 1;
        if unwarned
            .warned_at
            .is_some_and(|warned_at| now.duration_since(warned_at) < WARNING_INTERVAL)
        {
            return None;
        }
        unwarned.warned_at = Some(now);
        Some(std::mem::take(&mut unwarned.count))
    }
}

impl Drop for PeerSlot {
    fn drop(&mut self) {
        let mut held = unpoisoned::lock(&self.peers);
        let Some(count) = held.get_mut(&self.peer) else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            held.remove(&self.peer);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv6Addr;

    use super::*;

    #[test]
    fn one_ipv6_64_holds_its_share_until_a_connection_ends() {
        let peers = Peers::default();
        let address = |last: u16| IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, last));

        let mut held: Vec<PeerSlot> = (0..64)
            .filter_map(|last| peers.admit(address(last)))
            .collect();

        assert_eq!(held.len(), PER_PEER);
        assert!(peers.admit(address(100)).is_none());
        assert!(peers.admit("192.0.2.1".parse().unwrap()).is_some());
        held.pop();
        assert!(peers.admit(address(100)).is_some());
        drop(held);
        assert!(unpoisoned::lock(&peers.0).is_empty());
    }

    #[test]
    fn a_refusal_frees_its_connection_permit_and_past_its_budget_gets_none() {
        let permits = Arc::new(Semaphore::new(1));
        let refusals = Refusals::default();

        let first = refusals.admit(Arc::clone(&permits).try_acquire_owned().unwrap());
        let mut answering: Vec<_> = (1..REFUSING_AT_ONCE)
            .filter_map(|_| refusals.admit(Arc::clone(&permits).try_acquire_owned().unwrap()))
            .collect();

        assert!(first.is_some());
        assert_eq!(answering.len(), REFUSING_AT_ONCE - 1);
        assert_eq!(permits.available_permits(), 1);
        assert!(
            refusals
                .admit(Arc::clone(&permits).try_acquire_owned().unwrap())
                .is_none()
        );
        answering.pop();
        assert!(
            refusals
                .admit(Arc::clone(&permits).try_acquire_owned().unwrap())
                .is_some()
        );
    }

    #[test]
    fn refusals_are_told_once_a_minute_with_their_count() {
        let refusals = Refusals::default();
        let start = Instant::now();

        let told: Vec<Option<u64>> = [0, 1, 59, 61, 62, 200]
            .into_iter()
            .map(|seconds| refusals.counted(start + Duration::from_secs(seconds)))
            .collect();

        assert_eq!(told, [Some(1), None, None, Some(3), None, Some(2)]);
    }
}
