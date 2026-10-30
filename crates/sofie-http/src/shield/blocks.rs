use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use sofie_auth::{Signed, SigningKeys};
use sofie_core::{Uuid, unpoisoned};

use super::rule::Rule;
use crate::throttle::Requester;

/// A few megabytes: past it a new block is refused, so a flood of addresses cannot exhaust memory.
const MAX_BLOCKS: usize = 65_536;
const ID_LENGTH: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    Caller(Requester),
    Account(Uuid),
    /// So one caller's failures lock the account for that caller alone.
    AccountFrom(Uuid, Requester),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Grade {
    Observe,
    Slow,
    Block,
    Lock,
}

/// `until` is in microseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Block {
    pub(crate) rule: Rule,
    pub(crate) grade: Grade,
    pub(crate) until: i64,
    pub(crate) shared: bool,
}

/// Written only by the detector, the shared table and `ShieldBlocks::lift`.
#[derive(Default)]
pub(crate) struct Blocks {
    table: RwLock<HashMap<Key, Block>>,
    /// Read without the lock, so a process with no block pays one load.
    size: AtomicUsize,
}

impl Grade {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Slow => "slow",
            Self::Block => "block",
            Self::Lock => "lock",
        }
    }

    pub(crate) fn named(name: &str) -> Option<Self> {
        [Self::Observe, Self::Slow, Self::Block, Self::Lock]
            .into_iter()
            .find(|grade| grade.name() == name)
    }
}

impl Key {
    pub(crate) fn stored(self) -> String {
        match self {
            Self::Caller(Requester::Principal(principal)) => format!("principal:{principal}"),
            Self::Caller(Requester::Address(address)) => format!("address:{address}"),
            Self::Account(account) => format!("account:{account}"),
            Self::AccountFrom(account, caller) => {
                format!("account_from:{account}/{}", Self::Caller(caller).stored())
            }
        }
    }

    pub(crate) fn parse(stored: &str) -> Option<Self> {
        let (kind, value) = stored.split_once(':')?;
        match kind {
            "principal" => Some(Self::Caller(Requester::Principal(value.parse().ok()?))),
            "address" => Some(Self::Caller(Requester::Address(value.parse().ok()?))),
            "account" => Some(Self::Account(value.parse().ok()?)),
            "account_from" => {
                let (account, caller) = value.split_once('/')?;
                match Self::parse(caller)? {
                    Self::Caller(caller) => Some(Self::AccountFrom(account.parse().ok()?, caller)),
                    Self::Account(_) | Self::AccountFrom(..) => None,
                }
            }
            _ => None,
        }
    }

    /// Keyed, so an address shown by its network is not found again by trying each one in it.
    pub(crate) fn id(self, keys: &SigningKeys) -> String {
        truncated(keys.sign(Signed::ShieldBlock, self.stored().as_bytes()))
    }

    fn is_named_by(self, id: &str, keys: &SigningKeys) -> bool {
        keys.signatures(Signed::ShieldBlock, self.stored().as_bytes())
            .any(|signature| truncated(signature) == id)
    }

    pub(crate) fn shown(self) -> String {
        match self {
            Self::Caller(Requester::Address(address)) => format!("address {}", network(address)),
            Self::Caller(Requester::Principal(principal)) => format!("principal {principal}"),
            Self::Account(account) => format!("account {account}"),
            Self::AccountFrom(account, caller) => {
                format!("account {account} from {}", Self::Caller(caller).shown())
            }
        }
    }
}

fn truncated(mut signature: String) -> String {
    signature.truncate(ID_LENGTH);
    signature
}

/// Enough to tell networks apart, too little to name a subscriber.
fn network(address: IpAddr) -> String {
    match address {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            format!("{a}.{b}.{c}.0/24")
        }
        IpAddr::V6(v6) => {
            let kept = v6.to_bits() & (u128::MAX << 80);
            format!("{}/48", std::net::Ipv6Addr::from_bits(kept))
        }
    }
}

impl Blocks {
    pub(crate) fn is_empty(&self) -> bool {
        self.size.load(Ordering::Relaxed) == 0
    }

    pub(crate) fn find(&self, key: Key, now: i64) -> Option<Block> {
        let table = unpoisoned::read(&self.table);
        table.get(&key).copied().filter(|block| block.until > now)
    }

    /// False when the table is full even of blocks in force.
    pub(crate) fn insert(&self, key: Key, block: Block, now: i64) -> bool {
        let mut table = unpoisoned::write(&self.table);
        if let Some(held) = table.get(&key)
            && held.grade > block.grade
            && held.until > now
        {
            return true;
        }
        if table.len() >= MAX_BLOCKS && !table.contains_key(&key) {
            table.retain(|_, block| block.until > now);
            if table.len() >= MAX_BLOCKS {
                return false;
            }
        }
        table.insert(key, block);
        self.size.store(table.len(), Ordering::Relaxed);
        true
    }

    /// Found under the read lock, as hashing every key would hold up every request's lookup.
    pub(crate) fn lift(&self, id: &str, keys: &SigningKeys) -> Option<Key> {
        let key = unpoisoned::read(&self.table)
            .keys()
            .copied()
            .find(|key| key.is_named_by(id, keys))?;
        let mut table = unpoisoned::write(&self.table);
        table.remove(&key)?;
        self.size.store(table.len(), Ordering::Relaxed);
        Some(key)
    }

    pub(crate) fn sweep(&self, now: i64) {
        if self.is_empty() {
            return;
        }
        let mut table = unpoisoned::write(&self.table);
        table.retain(|_, block| block.until > now);
        self.size.store(table.len(), Ordering::Relaxed);
    }

    pub(crate) fn active(&self, now: i64) -> Vec<(Key, Block)> {
        let table = unpoisoned::read(&self.table);
        let mut active: Vec<(Key, Block)> = table
            .iter()
            .filter(|(_, block)| block.until > now)
            .map(|(key, block)| (*key, *block))
            .collect();
        active.sort_by_key(|(_, block)| block.until);
        active
    }

    /// A shared block no longer in the table was lifted elsewhere.
    pub(crate) fn follow_shared(&self, shared: &[(Key, Block)], now: i64) {
        let held: HashSet<Key> = shared.iter().map(|(key, _)| *key).collect();
        let mut table = unpoisoned::write(&self.table);
        table.retain(|key, block| !block.shared || held.contains(key));
        for (key, block) in shared {
            if table.len() < MAX_BLOCKS || table.contains_key(key) {
                table.insert(*key, *block);
            }
        }
        table.retain(|_, block| block.until > now);
        self.size.store(table.len(), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(text: &str) -> Key {
        Key::Caller(Requester::of(None, text.parse().unwrap()))
    }

    fn keys(byte: u8) -> SigningKeys {
        SigningKeys::new(&[[byte; 32]]).unwrap()
    }

    fn block(grade: Grade, until: i64) -> Block {
        Block {
            rule: Rule::Scan,
            grade,
            until,
            shared: false,
        }
    }

    #[test]
    fn a_block_in_force_is_found_and_an_expired_one_is_not() {
        let blocks = Blocks::default();
        let (slowed, blocked) = (address("192.0.2.1"), address("192.0.2.2"));
        assert!(blocks.is_empty());

        blocks.insert(slowed, block(Grade::Slow, 100), 0);
        blocks.insert(blocked, block(Grade::Block, 50), 0);

        assert_eq!(blocks.find(blocked, 10).unwrap().grade, Grade::Block);
        assert_eq!(blocks.find(slowed, 60).unwrap().grade, Grade::Slow);
        assert_eq!(blocks.find(blocked, 60), None);
        blocks.sweep(60);
        assert_eq!(blocks.active(60).len(), 1);
    }

    #[test]
    fn a_lifted_block_is_gone_and_keys_read_back_as_stored() {
        let blocks = Blocks::default();
        let key = address("2001:db8:1:2:3::9");
        blocks.insert(key, block(Grade::Block, 100), 0);
        let id = blocks.active(0)[0].0.id(&keys(1));

        assert_eq!(Key::parse(&key.stored()), Some(key));
        assert_eq!(key.shown(), "address 2001:db8:1::/48");
        assert_eq!(address("198.51.100.77").shown(), "address 198.51.100.0/24");
        let Key::Caller(caller) = key else {
            unreachable!()
        };
        let account_from = Key::AccountFrom(Uuid::now_v7().unwrap(), caller);
        assert_eq!(Key::parse(&account_from.stored()), Some(account_from));
        assert_eq!(blocks.lift("unknown", &keys(1)), None);
        assert_eq!(blocks.lift(&id, &keys(1)), Some(key));
        assert!(blocks.is_empty());
    }

    #[test]
    fn a_block_listed_by_one_instance_is_lifted_by_another() {
        let (one, another) = (Blocks::default(), Blocks::default());
        let (first, second) = (address("192.0.2.1"), address("192.0.2.2"));
        let shared = Block {
            shared: true,
            ..block(Grade::Block, 100)
        };
        one.follow_shared(&[(first, shared), (second, shared)], 0);
        another.follow_shared(&[(second, shared), (first, shared)], 0);

        let listed = one.active(0)[0].0;

        assert_eq!(another.lift(&listed.id(&keys(1)), &keys(1)), Some(listed));
        assert_eq!(another.active(0).len(), 1);
    }

    #[test]
    fn a_block_listed_before_a_key_rotation_is_still_lifted() {
        let blocks = Blocks::default();
        let key = address("192.0.2.1");
        blocks.insert(key, block(Grade::Block, 100), 0);
        let listed = key.id(&keys(1));
        let rotated = SigningKeys::new(&[[2; 32], [1; 32]]).unwrap();

        assert_eq!(blocks.lift(&listed, &keys(2)), None);
        assert_eq!(blocks.lift(&listed, &rotated), Some(key));
    }

    #[test]
    fn a_block_id_says_nothing_without_the_apps_keys() {
        let key = address("198.51.100.77");

        assert_eq!(key.id(&keys(1)).len(), ID_LENGTH);
        assert_eq!(key.id(&keys(1)), key.id(&keys(1)));
        assert_ne!(key.id(&keys(1)), key.id(&keys(2)));
    }

    #[test]
    fn a_shared_block_lifted_elsewhere_is_lifted_here_and_a_local_one_kept() {
        let blocks = Blocks::default();
        let (local, shared) = (address("192.0.2.1"), address("192.0.2.2"));
        blocks.insert(local, block(Grade::Block, 100), 0);
        let held = Block {
            shared: true,
            ..block(Grade::Block, 100)
        };

        blocks.follow_shared(&[(shared, held)], 0);
        assert_eq!(blocks.active(0).len(), 2);
        blocks.follow_shared(&[], 0);

        assert!(blocks.find(local, 0).is_some());
        assert_eq!(blocks.find(shared, 0), None);
        assert_eq!(blocks.active(0).len(), 1);
    }
}
