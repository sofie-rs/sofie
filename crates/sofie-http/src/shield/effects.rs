use std::sync::{Arc, Weak};
use std::time::Duration;

use sofie_core::{Clock, Error, Result, Timestamp};
use sofie_db::{Channel, Db, Listening, SharedBlock, SharedBlocks};
use tokio::runtime::Handle;

use super::blocks::{Block, Blocks, Grade, Key};
use super::detector::Decision;
use super::rule::Rule;
use crate::AreaMembers;

/// When no wake-up came, or none can.
const FOLLOW_INTERVAL: Duration = Duration::from_secs(10);

pub(crate) struct Effects {
    pub(crate) runtime: Handle,
    pub(crate) db: Db,
    pub(crate) members: Option<Arc<dyn AreaMembers>>,
    pub(crate) shared: bool,
}

impl Effects {
    pub(crate) fn decided(&self, decision: &Decision) {
        if let (Grade::Lock, Key::Account(account) | Key::AccountFrom(account, _), Some(members)) =
            (decision.grade, decision.key, &self.members)
        {
            let (db, members, until) = (self.db.clone(), Arc::clone(members), decision.until);
            self.runtime.spawn(async move {
                let told = match timestamp(until) {
                    Ok(until) => members.sign_in_locked(&db, account, until).await,
                    Err(error) => Err(error),
                };
                if let Err(error) = told {
                    tracing::error!(error = %error, shield.account = %account, "telling an account's owner of its sign-in lock failed");
                }
            });
        }
        if self.shared {
            let (db, decision) = (self.db.clone(), *decision);
            self.runtime.spawn(async move {
                if let Err(error) = share(&db, &decision).await {
                    tracing::error!(error = %error, shield.rule = decision.rule.name(), "sharing a shield block failed");
                }
            });
        }
    }
}

async fn share(db: &Db, decision: &Decision) -> Result<()> {
    let block = SharedBlock {
        key: decision.key.stored(),
        rule: decision.rule.name().to_owned(),
        grade: decision.grade.name().to_owned(),
        until: timestamp(decision.until)?,
    };
    SharedBlocks::of(db).share(&block).await
}

fn timestamp(micros: i64) -> Result<Timestamp> {
    Timestamp::from_microsecond(micros)
        .map_err(|error| Error::internal("a shield block's end is out of range", error))
}

pub(crate) async fn follow(db: Db, clock: Clock, blocks: Weak<Blocks>) {
    let mut listening = Listening::start(db.clone(), Channel::ShieldBlocks, FOLLOW_INTERVAL).await;
    loop {
        let Some(held) = blocks.upgrade() else {
            return;
        };
        let now = clock.now().timestamp();
        match SharedBlocks::of(&db).active(now).await {
            Ok(rows) => held.follow_shared(&followed(&rows), now.as_microsecond()),
            Err(error) => tracing::warn!(error = %error, "reading the shared shield blocks failed"),
        }
        drop(held);
        listening.changed().await;
    }
}

/// A row this version cannot read is skipped, said once per reading.
fn followed(rows: &[SharedBlock]) -> Vec<(Key, Block)> {
    let blocks: Vec<(Key, Block)> = rows.iter().filter_map(block_of).collect();
    if blocks.len() < rows.len() {
        tracing::warn!(
            shield.skipped = rows.len() - blocks.len(),
            "shared shield blocks this version cannot read were skipped"
        );
    }
    blocks
}

fn block_of(row: &SharedBlock) -> Option<(Key, Block)> {
    let block = Block {
        id: 0,
        rule: Rule::named(&row.rule)?,
        grade: Grade::named(&row.grade)?,
        until: row.until.as_microsecond(),
        shared: true,
    };
    Some((Key::parse(&row.key)?, block))
}

pub(crate) async fn lift_shared(db: &Db, key: Key) -> Result<()> {
    SharedBlocks::of(db).lift(&key.stored()).await
}
