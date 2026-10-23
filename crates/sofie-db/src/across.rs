use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use sofie_core::{Error, FieldError, Result, SqlValue, Uuid};

use crate::statement::{Criteria, ONE};
use crate::{Aggregate, Condition, ExpectedRows, Id, Outcome, Plan, Read, Step, Unmet};

/// Whether records match across every owner, without any of them leaving: no rows, fields or ids.
pub struct Across<A: Aggregate> {
    read: Read,
    criteria: Criteria,
    marker: PhantomData<fn() -> A>,
}

impl<A: Aggregate> Across<A> {
    /// Only for the derive's crate-private `across`: called elsewhere, it escapes that limit.
    #[doc(hidden)]
    #[must_use]
    pub fn for_derived_across_only(read: &Read) -> Self {
        Self {
            read: read.across_owners(),
            criteria: Criteria::default(),
            marker: PhantomData,
        }
    }

    #[must_use]
    pub fn where_(mut self, condition: Condition<A>) -> Self {
        self.criteria.conditions.push(condition.kind);
        self
    }

    pub async fn exists(&self) -> Result<bool> {
        let matched = self.criteria.clone().exists::<A>(&self.read).await?;
        emit(
            A::MODULE,
            A::NAME,
            self.read.actor(),
            CheckKind::Exists,
            matched,
        );
        Ok(matched)
    }

    pub async fn count(&self) -> Result<u64> {
        let count = self.criteria.clone().count::<A>(&self.read).await?;
        emit(
            A::MODULE,
            A::NAME,
            self.read.actor(),
            CheckKind::Count,
            count > 0,
        );
        Ok(count)
    }

    /// The same check, run again inside the commit after serializing on the aggregate.
    #[must_use]
    pub fn guard(self, refusal: FieldError) -> Guard<A> {
        Guard {
            criteria: self.criteria,
            refusal,
            key: format!("{}.{}", A::MODULE, A::NAME),
            marker: PhantomData,
        }
    }
}

/// Refuses its commit with its field error when a matching record exists by then; replies nothing.
pub struct Guard<A: Aggregate> {
    criteria: Criteria,
    refusal: FieldError,
    key: String,
    marker: PhantomData<fn() -> A>,
}

impl<A: Aggregate> Guard<A> {
    #[must_use]
    pub fn except(mut self, id: Id<A>) -> Self {
        self.criteria.conditions.push(A::ID.ne(id).kind);
        self
    }

    /// Any two commits whose records could clash must share the key.
    #[must_use]
    pub fn keyed(mut self, key: impl fmt::Display) -> Self {
        self.key = format!("{}.{}:{key}", A::MODULE, A::NAME);
        self
    }
}

impl<A: Aggregate> Outcome for Guard<A> {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let dialect = plan.dialect();
        let mut steps = Vec::with_capacity(2);
        if let Some(lock) = dialect.key_lock() {
            steps.push(Step::statement(
                lock,
                vec![SqlValue::Text(self.key.clone())],
                None,
            ));
        }
        let criteria = Criteria {
            limit: Some(1),
            ..self.criteria.clone()
        };
        let (sql, params) = criteria.select_sql::<A>(dialect, None, ONE);
        let check = Arc::new(GuardCheck {
            error: self.refusal.clone(),
            module: A::MODULE,
            aggregate: A::NAME,
            actor: plan.reach().actor(),
            state: AtomicU8::new(UNCHECKED),
        });
        steps.push(Step::Statement {
            sql,
            params,
            expect_rows: Some(ExpectedRows {
                rows: 0,
                otherwise: Unmet::Refused(Arc::clone(&check)),
            }),
        });
        plan.guard(steps, check);
        Ok(())
    }
}

const UNCHECKED: u8 = 0;
const CLEAR: u8 = 1;
const MATCHED: u8 = 2;

/// Keeps its outcome for the committing task to audit: a driver may check on another thread.
#[derive(Debug)]
pub struct GuardCheck {
    error: FieldError,
    module: &'static str,
    aggregate: &'static str,
    actor: Option<Uuid>,
    state: AtomicU8,
}

impl GuardCheck {
    pub(crate) fn checked(&self, matched: bool) -> Result<()> {
        self.state
            .store(if matched { MATCHED } else { CLEAR }, Ordering::Relaxed);
        if matched {
            return Err(Error::Invalid(vec![self.error.clone()]));
        }
        Ok(())
    }

    pub(crate) fn audit(&self) {
        let matched = match self.state.load(Ordering::Relaxed) {
            CLEAR => false,
            MATCHED => true,
            _ => return,
        };
        emit(
            self.module,
            self.aggregate,
            self.actor,
            CheckKind::Guard,
            matched,
        );
    }
}

impl PartialEq for GuardCheck {
    fn eq(&self, other: &Self) -> bool {
        (&self.error, self.module, self.aggregate, self.actor)
            == (&other.error, other.module, other.aggregate, other.actor)
    }
}

impl Eq for GuardCheck {}

#[derive(Clone, Copy)]
enum CheckKind {
    Exists,
    Count,
    Guard,
}

impl CheckKind {
    const fn name(self) -> &'static str {
        match self {
            Self::Exists => "exists",
            Self::Count => "count",
            Self::Guard => "guard",
        }
    }
}

/// Never the conditions' values: they may be another owner's data.
fn emit(
    module: &'static str,
    aggregate: &'static str,
    actor: Option<Uuid>,
    kind: CheckKind,
    matched: bool,
) {
    tracing::info!(
        module,
        aggregate,
        actor = actor.map(tracing::field::display),
        kind = kind.name(),
        outcome = if matched { "matched" } else { "clear" },
        "cross_owner_check"
    );
}

#[cfg(test)]
mod tests {
    use sofie_core::Instant;

    use super::*;
    use crate::aggregate::tests::{NUMBERED, Note};
    use crate::{Column, Meta, Reach};

    const TITLE: Column<Note, String> = Column::new("title");

    fn guard() -> Guard<Note> {
        Across::<Note> {
            read: never_read(),
            criteria: Criteria::default(),
            marker: PhantomData,
        }
        .where_(TITLE.eq("taken".to_owned()))
        .guard(FieldError::invalid("title", "is taken"))
    }

    fn never_read() -> Read {
        Read::within(&crate::Db::new(NoDriver), Reach::All)
    }

    struct NoDriver;

    impl crate::Driver for NoDriver {
        fn dialect(&self) -> &dyn crate::Dialect {
            &NUMBERED
        }

        fn query<'a>(
            &'a self,
            _sql: &'a str,
            _params: &'a [SqlValue],
            _rows: &'a mut dyn crate::RowSink,
        ) -> crate::BoxFuture<'a, Result<()>> {
            unimplemented!("a planning test reads nothing")
        }

        fn transaction(&self, _steps: Vec<Step>) -> crate::BoxFuture<'_, Result<()>> {
            unimplemented!("a planning test commits nothing")
        }
    }

    fn planned(outcome: &impl Outcome, reach: Reach, leading: Vec<Step>) -> Vec<Step> {
        let now = Instant::from(sofie_core::Timestamp::from_second(1_000).unwrap());
        let mut plan = Plan::new(&NUMBERED, reach, now, leading, None);
        outcome.plan(&mut plan).unwrap();
        plan.into_parts().unwrap().steps
    }

    fn sql(step: &Step) -> &str {
        match step {
            Step::Statement { sql, .. } | Step::Script(sql) => sql,
        }
    }

    #[test]
    fn a_guard_listed_last_is_checked_after_the_leading_steps_and_before_any_write() {
        let note = Note {
            meta: Meta::new().unwrap(),
            title: "taken".to_owned(),
        };
        let claim = Step::statement("INSERT INTO claims", Vec::new(), Some(1));

        let steps = planned(&(note, guard()), Reach::All, vec![claim]);

        let order: Vec<&str> = steps.iter().map(sql).collect();
        assert_eq!(order[0], "INSERT INTO claims");
        assert!(
            order[1].starts_with("SELECT 1 FROM \"notes\".\"note\""),
            "{order:?}"
        );
        assert!(
            order[2].starts_with("INSERT INTO \"notes\".\"note\""),
            "{order:?}"
        );
    }

    #[test]
    fn a_guard_checks_every_owner_s_records_except_those_it_names() {
        let own = Id::<Note>::from_uuid(Uuid::from_u128(3));

        let steps = planned(
            &guard().except(own),
            Reach::OwnedBy(Uuid::from_u128(7)),
            Vec::new(),
        );

        let [
            Step::Statement {
                sql,
                params,
                expect_rows: Some(expected),
            },
        ] = &steps[..]
        else {
            panic!(
                "expected the check alone, without a lock where commits run one at a time: {steps:?}"
            );
        };
        assert_eq!(
            sql,
            "SELECT 1 FROM \"notes\".\"note\" WHERE (\"deleted_at\" IS NULL) AND (\"title\" = $1) \
             AND NOT (\"id\" = $2) LIMIT $3"
        );
        assert_eq!(params[1], SqlValue::Uuid(Uuid::from_u128(3)));
        assert_eq!(expected.rows, 0);
    }

    #[test]
    fn a_matching_row_refuses_with_the_guard_s_field_error_and_none_lets_it_pass() {
        let steps = planned(&guard(), Reach::All, Vec::new());
        let Step::Statement {
            expect_rows: Some(expected),
            ..
        } = &steps[0]
        else {
            panic!("expected the check: {steps:?}");
        };

        let refused = expected.check(1);

        assert!(
            matches!(&refused, Err(Error::Invalid(fields))
                if fields[0].field == "title" && fields[0].message == "is taken"),
            "{refused:?}"
        );
        assert!(expected.check(0).is_ok());
    }
}
