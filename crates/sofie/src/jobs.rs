use std::fmt::Write as _;

use sofie_core::{Error, Result, Uuid};
use sofie_db::{Db, DeadMessage, DeadMessages};

const LISTED: u64 = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Jobs {
    Dead,
    Replay(Uuid),
}

impl Jobs {
    pub(crate) fn parse_replay(id: &str) -> Result<Self> {
        id.parse::<Uuid>().map(Self::Replay).map_err(|error| {
            Error::internal(
                format!("`{id}` is not a message id; `jobs dead` lists them"),
                error,
            )
        })
    }

    pub(crate) async fn run(&self, db: Db) -> Result<()> {
        let messages = DeadMessages::new(db);
        match self {
            Self::Dead => {
                print!("{}", listing(&messages.newest(LISTED).await?));
                Ok(())
            }
            Self::Replay(id) => {
                let replayed = messages.replay(*id).await?;
                println!(
                    "replayed {} ({}) after {} attempts; the worker runs it next",
                    replayed.id, replayed.name, replayed.attempts
                );
                Ok(())
            }
        }
    }
}

fn listing(dead: &[DeadMessage]) -> String {
    if dead.is_empty() {
        return "no dead messages\n".to_owned();
    }
    let mut text = String::new();
    for message in dead {
        let error = message.last_error.as_deref().unwrap_or("");
        let _ = writeln!(
            text,
            "{}  {}  {}  {} attempts  {}",
            message.id,
            message.dead_at,
            message.name,
            message.attempts,
            error.lines().next().unwrap_or("")
        );
    }
    if u64::try_from(dead.len()).is_ok_and(|shown| shown == LISTED) {
        let _ = writeln!(text, "the {LISTED} most recently dead are shown");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_dead_message_shows_on_one_line_with_its_error_s_first_line() {
        let message = DeadMessage {
            id: Uuid::from_u128(1),
            name: "order.ship".to_owned(),
            attempts: 25,
            last_error: Some("the carrier refused\nat line 2".to_owned()),
            dead_at: "2026-10-22T09:00:00Z".parse().unwrap(),
        };

        assert_eq!(
            listing(&[message]),
            "00000000-0000-0000-0000-000000000001  2026-10-22T09:00:00Z  order.ship  25 attempts  \
             the carrier refused\n"
        );
        assert_eq!(listing(&[]), "no dead messages\n");
    }
}
