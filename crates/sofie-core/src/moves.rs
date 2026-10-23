use std::cell::RefCell;
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

use serde::ser::SerializeMap as _;
use serde::{Serialize, Serializer};

use crate::show::{Fields, Shown};
use crate::{Cast, Error, FieldError, Result, SqlValue};

const REFUSED_MOVE: &str = "validation.move";

/// `None` for a value that holds no state, such as an absent `Option`.
pub type Moves<T> = fn(&T) -> Option<&'static Targets>;

/// The states a value may move to, each `true`, as `<field>_to` shows and serializes them.
#[derive(Debug, PartialEq, Eq)]
pub struct Targets {
    pub of: &'static str,
    pub states: &'static [&'static str],
}

impl Targets {
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.states.contains(&name)
    }
}

#[must_use]
pub fn refused(field: impl Into<String>, from: &str, to: &str) -> Error {
    let error = FieldError::new(
        field,
        REFUSED_MOVE,
        format!("cannot move from {from} to {to}"),
    )
    .with("from", from)
    .with("to", to);
    Error::Invalid(vec![error])
}

#[must_use]
pub fn targets<T: Cast>(value: &T) -> Option<&'static Targets> {
    T::MOVES.and_then(|moves| moves(value))
}

/// `Cast::refuse_unlisted_move` of a value holding at most one state, by `Cast::MOVES`.
pub fn refuse_unlisted<T: Cast>(field: &str, stored: &T, current: &T) -> Result<()> {
    let Some(targets) = targets(stored) else {
        return Ok(());
    };
    let (mut old, mut new) = (Vec::new(), Vec::new());
    stored.encode(&mut old);
    current.encode(&mut new);
    if new == old {
        return Ok(());
    }
    match new.as_slice() {
        // Taken first, so a listed move also uses up the record's own journal entry.
        [SqlValue::Text(target)]
            if take(targets.of, state_name(&old), target) || targets.contains(target) =>
        {
            Ok(())
        }
        _ => Err(refused(field, state_name(&old), state_name(&new))),
    }
}

fn state_name(columns: &[SqlValue]) -> &str {
    match columns {
        [SqlValue::Text(name)] => name,
        _ => "nothing",
    }
}

impl Fields for Targets {
    fn name(&self, index: usize) -> Option<&str> {
        self.states.get(index).copied()
    }

    fn get(&self, name: &str) -> Option<Shown<'_>> {
        self.contains(name).then_some(Shown::Bool(true))
    }
}

impl Serialize for Targets {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.states.len()))?;
        for name in self.states {
            map.serialize_entry(name, &true)?;
        }
        map.end()
    }
}

/// Outside `recorded` moves land in the thread's own journal, which keeps only the latest.
const UNSCOPED_LIMIT: usize = 64;

thread_local! {
    static JOURNAL: RefCell<Journal> = const {
        RefCell::new(Journal {
            moves: Vec::new(),
            scoped: false,
        })
    };
}

struct Journal {
    moves: Vec<Move>,
    /// A scope's journal ends with its future, so it keeps every move.
    scoped: bool,
}

#[derive(Clone, Copy)]
struct Move {
    of: &'static str,
    from: &'static str,
    to: &'static str,
}

pub(crate) fn record(of: &'static str, from: &'static str, to: &'static str) {
    JOURNAL.with_borrow_mut(|journal| {
        if !journal.scoped && journal.moves.len() >= UNSCOPED_LIMIT {
            journal.moves.remove(0);
        }
        journal.moves.push(Move { of, from, to });
    });
}

/// A value carries no record, so the moves through `Variants::to` that explain a save are taken
/// out: each explains one record's change, never another's.
#[must_use]
pub fn take(of: &str, from: &str, to: &str) -> bool {
    JOURNAL.with_borrow_mut(|journal| {
        let Some(mut chain) = chain(&journal.moves, of, from, to) else {
            return false;
        };
        chain.sort_unstable_by(|first, second| second.cmp(first));
        for index in chain {
            journal.moves.remove(index);
        }
        true
    })
}

fn chain(moves: &[Move], of: &str, from: &str, to: &str) -> Option<Vec<usize>> {
    let mut before: Vec<Option<usize>> = vec![None; moves.len()];
    let mut seen: Vec<bool> = moves
        .iter()
        .map(|step| step.of == of && step.from == from)
        .collect();
    let mut queue: VecDeque<usize> = (0..moves.len()).filter(|index| seen[*index]).collect();
    while let Some(index) = queue.pop_front() {
        let step = moves[index];
        if step.to == to {
            let mut chain = vec![index];
            while let Some(previous) = before[*chain.last()?] {
                chain.push(previous);
            }
            return Some(chain);
        }
        for (next, candidate) in moves.iter().enumerate() {
            if !seen[next] && candidate.of == of && candidate.from == step.to {
                seen[next] = true;
                before[next] = Some(index);
                queue.push_back(next);
            }
        }
    }
    None
}

/// Keeps the moves made while `future` runs with it, across threads and away from other tasks.
pub fn recorded<F: Future>(future: F) -> Recorded<F> {
    Recorded {
        future: Box::pin(future),
        journal: Journal {
            moves: Vec::new(),
            scoped: true,
        },
    }
}

pub struct Recorded<F: Future> {
    future: Pin<Box<F>>,
    journal: Journal,
}

impl<F: Future> Future for Recorded<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<F::Output> {
        let this = &mut *self;
        let _lent = Lent::new(&mut this.journal);
        this.future.as_mut().poll(context)
    }
}

/// Gives the thread its own journal back in `Drop`, so a panicking future, which a worker
/// catches, does not leave its journal on the thread.
struct Lent<'a>(&'a mut Journal);

impl<'a> Lent<'a> {
    fn new(journal: &'a mut Journal) -> Self {
        swap_with_thread(journal);
        Self(journal)
    }
}

impl Drop for Lent<'_> {
    fn drop(&mut self) {
        swap_with_thread(self.0);
    }
}

fn swap_with_thread(journal: &mut Journal) {
    JOURNAL.with_borrow_mut(|thread| std::mem::swap(thread, journal));
}

#[cfg(test)]
mod tests {
    use std::task::Waker;

    use super::*;

    fn in_scope<T>(work: impl FnOnce() -> T) -> T {
        let mut scoped = std::pin::pin!(recorded(async { work() }));
        match scoped
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(answer) => answer,
            Poll::Pending => unreachable!("the work never waits"),
        }
    }

    #[test]
    fn targets_show_and_serialize_each_state_as_true() {
        let targets = Targets {
            of: "Light",
            states: &["RED"],
        };

        assert!(targets.contains("RED") && !targets.contains("GREEN"));
        assert_eq!((targets.name(0), targets.name(1)), (Some("RED"), None));
        assert!(matches!(targets.get("RED"), Some(Shown::Bool(true))));
        assert!(targets.get("GREEN").is_none());
        assert_eq!(
            serde_json::to_value(&targets).unwrap(),
            serde_json::json!({ "RED": true })
        );
    }

    #[test]
    fn follows_recorded_steps_of_one_type() {
        in_scope(|| {
            record("Order", "OPEN", "PAID");
            record("Order", "PAID", "SHIPPED");
            record("Light", "SHIPPED", "GONE");

            assert!(!take("Order", "OPEN", "GONE"));
            assert!(!take("Order", "SHIPPED", "OPEN"));
            assert!(take("Order", "OPEN", "SHIPPED"));
        });
    }

    #[test]
    fn a_move_of_another_type_explains_nothing() {
        in_scope(|| {
            record("Payment", "PENDING", "PAID");

            assert!(!take("Order", "PENDING", "PAID"));
            assert!(take("Payment", "PENDING", "PAID"));
        });
    }

    #[test]
    fn a_move_explains_one_record_s_save_only() {
        in_scope(|| {
            record("Order", "OPEN", "PAID");
            record("Order", "PAID", "SHIPPED");

            assert!(take("Order", "OPEN", "PAID"), "the first record's own move");
            assert!(
                take("Order", "PAID", "SHIPPED"),
                "the second record's own move"
            );
            assert!(
                !take("Order", "OPEN", "SHIPPED"),
                "a third record jumping by assignment"
            );
        });
    }

    #[test]
    fn a_state_is_not_reached_from_itself_without_a_round_trip() {
        in_scope(|| {
            record("Order", "OPEN", "PAID");

            assert!(!take("Order", "OPEN", "OPEN"));
            record("Order", "PAID", "OPEN");
            assert!(take("Order", "OPEN", "OPEN"));
        });
    }

    #[test]
    fn a_scope_keeps_every_move_however_many() {
        in_scope(|| {
            record("Order", "OPEN", "PAID");
            for _ in 0..UNSCOPED_LIMIT * 2 {
                record("Light", "RED", "GREEN");
            }

            assert!(take("Order", "OPEN", "PAID"));
        });
    }

    #[test]
    fn a_recorded_future_keeps_its_moves_to_itself() {
        let inside = in_scope(|| {
            record("Order", "OPEN", "PAID");
            take("Order", "OPEN", "PAID")
        });

        assert!(inside);
        assert!(!take("Order", "OPEN", "PAID"));
    }

    #[test]
    fn a_panicking_future_gives_the_thread_its_own_journal_back() {
        let caught = std::panic::catch_unwind(|| {
            in_scope(|| {
                record("Order", "OPEN", "PAID");
                panic!("the handler failed");
            });
        });

        assert!(caught.is_err());
        assert!(!take("Order", "OPEN", "PAID"));
    }
}
