use std::borrow::Cow;
use std::sync::Arc;

use serde_json::Value;
use sofie_core::{Error, FieldError, Instant, Result, Uuid};

use crate::heard::Known;
use crate::messages::queue_steps;
use crate::{Dialect, GuardCheck, MessageKind, RaisedEvent, Reach, Reply, Step, UniqueViolation};

pub struct Plan<'d> {
    dialect: &'d dyn Dialect,
    reach: Reach,
    stamp: Stamp,
    steps: Vec<Step>,
    leading: usize,
    guards: Vec<Step>,
    guard_checks: Vec<Arc<GuardCheck>>,
    pub(crate) events: Vec<RaisedEvent>,
    unique_fields: Vec<UniqueField>,
    queues_messages: bool,
    pub(crate) heard: Option<Arc<Known>>,
    pub(crate) announcements: crate::commit::Announcements,
    #[cfg(feature = "module-roles")]
    role: Option<&'static str>,
    #[cfg(feature = "search-engine")]
    pub(crate) indexes_searches: bool,
}

pub(crate) struct Planned {
    pub(crate) steps: Vec<Step>,
    pub(crate) guard_checks: Vec<Arc<GuardCheck>>,
    pub(crate) unique_fields: Vec<UniqueField>,
    pub(crate) queues_messages: bool,
    pub(crate) announcements: crate::commit::Announcements,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub now: Instant,
    pub actor: Option<Uuid>,
    pub action: Option<&'static str>,
}

const UNIQUE_KEY: &str = "validation.unique";
#[cfg(feature = "module-roles")]
pub(crate) const FRAMEWORK_MODULE: &str = "sofie";
const TAKEN: &str = "is already taken";

pub(crate) struct UniqueField {
    index: Cow<'static, str>,
    /// Where known, the only prefix a database without schemas may add to the index's name.
    module: Option<&'static str>,
    field: &'static str,
    message: &'static str,
}

impl<'d> Plan<'d> {
    pub(crate) fn new(
        dialect: &'d dyn Dialect,
        reach: Reach,
        now: Instant,
        steps: Vec<Step>,
        heard: Option<Arc<Known>>,
    ) -> Self {
        Self {
            dialect,
            reach,
            stamp: Stamp {
                now,
                actor: reach.actor(),
                action: crate::action::current(),
            },
            leading: steps.len(),
            steps,
            guards: Vec::new(),
            guard_checks: Vec::new(),
            events: Vec::new(),
            unique_fields: Vec::new(),
            queues_messages: false,
            heard,
            announcements: crate::commit::Announcements::default(),
            #[cfg(feature = "module-roles")]
            role: None,
            #[cfg(feature = "search-engine")]
            indexes_searches: false,
        }
    }

    /// The framework's own records are written as the app's login, which owns them.
    #[cfg(feature = "module-roles")]
    pub(crate) fn act_as(&mut self, module: &'static str) {
        let role = (module != FRAMEWORK_MODULE).then_some(module);
        if role == self.role {
            return;
        }
        if let Some(statement) = self.dialect.enter_role(role) {
            self.steps.push(Step::Script(statement));
        }
        self.role = role;
    }

    #[must_use]
    pub fn dialect(&self) -> &'d dyn Dialect {
        self.dialect
    }

    #[must_use]
    pub fn reach(&self) -> Reach {
        self.reach
    }

    #[must_use]
    pub fn stamp(&self) -> Stamp {
        self.stamp
    }

    /// Runs as the module the plan last wrote for; [`Plan::push_as`] names it.
    pub fn push(&mut self, step: Step) {
        self.steps.push(step);
    }

    /// A raw step on `A`'s tables, run as `A`'s module whatever the outcomes before it wrote.
    pub fn push_as<A: crate::Aggregate>(&mut self, step: Step) {
        #[cfg(feature = "module-roles")]
        self.act_as(A::MODULE);
        self.steps.push(step);
    }

    pub(crate) fn guard(&mut self, steps: Vec<Step>, check: Arc<GuardCheck>) {
        self.guards.extend(steps);
        self.guard_checks.push(check);
    }

    pub fn raise(&mut self, events: &[RaisedEvent]) {
        self.events.extend_from_slice(events);
    }

    #[cfg(any(feature = "contracts", feature = "publishing", feature = "retention"))]
    pub(crate) fn raise_one(&mut self, name: &'static str, payload: Value) {
        self.events.push(RaisedEvent { name, payload });
    }

    pub(crate) fn queue<'a>(
        &mut self,
        kind: MessageKind,
        messages: impl IntoIterator<Item = (&'a str, Value)>,
    ) -> Result<()> {
        self.queue_at(kind, messages, self.stamp.now.timestamp())
    }

    pub(crate) fn queue_at<'a>(
        &mut self,
        kind: MessageKind,
        messages: impl IntoIterator<Item = (&'a str, Value)>,
        at: sofie_core::Timestamp,
    ) -> Result<()> {
        let heard = self.heard.as_deref();
        let queuing = self.stamp.queuing(kind, at);
        self.queues_messages |=
            queue_steps(self.dialect, &queuing, messages, heard, &mut self.steps)?;
        Ok(())
    }

    pub(crate) fn push_message(&mut self, step: Step) {
        self.queues_messages = true;
        self.steps.push(step);
    }

    /// Announces rows of `A` a raw [`Step::statement`] wrote, so caches and live screens refresh.
    #[cfg_attr(not(feature = "live"), expect(unused_variables))]
    pub fn touched<A: crate::Aggregate>(&mut self, owner: Option<Uuid>) {
        #[cfg(feature = "cache")]
        self.announcements.changed(crate::ChangedRecord {
            module: A::MODULE,
            aggregate: A::NAME,
            id: None,
        });
        #[cfg(feature = "live")]
        self.announcements
            .live_change(crate::TableChange::of::<A>(owner, self.reach));
    }

    /// Catches a duplicate another request committed after the handler's own check.
    pub fn unique(&mut self, index: &'static str, field: &'static str, message: &'static str) {
        self.unique_fields.push(UniqueField {
            index: Cow::Borrowed(index),
            module: None,
            field,
            message,
        });
    }

    /// `<table>_<column>`, or `<table>_<column>_key` as Postgres names an inline `UNIQUE`.
    pub(crate) fn unique_column(&mut self, module: &'static str, index: &str, field: &'static str) {
        self.unique_fields.push(UniqueField {
            index: Cow::Owned(index.to_owned()),
            module: Some(module),
            field,
            message: TAKEN,
        });
    }

    pub(crate) fn into_parts(mut self) -> Result<Planned> {
        let events = std::mem::take(&mut self.events);
        // A listener's job is queued with `ON CONFLICT`, which needs SELECT a module role lacks.
        #[cfg(feature = "module-roles")]
        if !events.is_empty() {
            self.act_as(FRAMEWORK_MODULE);
        }
        let named = events.into_iter().map(|event| (event.name, event.payload));
        self.queue(MessageKind::Event, named)?;
        let mut steps = self.steps;
        let writes = steps.split_off(self.leading);
        steps.extend(self.guards);
        steps.extend(writes);
        Ok(Planned {
            steps,
            guard_checks: self.guard_checks,
            unique_fields: self.unique_fields,
            queues_messages: self.queues_messages,
            announcements: self.announcements,
        })
    }
}

pub(crate) fn explain_duplicate(error: Error, unique_fields: &[UniqueField]) -> Error {
    let Error::Internal {
        source: Some(source),
        ..
    } = &error
    else {
        return error;
    };
    let Some(violation) = source.downcast_ref::<UniqueViolation>() else {
        return error;
    };
    let index = violation.index.as_str();
    let index = index.strip_suffix("_key").unwrap_or(index);
    // A database without schemas names the index after the module's table too.
    let prefixed = |unique: &&UniqueField| match unique.module {
        Some(module) => {
            index
                .strip_prefix(module)
                .and_then(|rest| rest.strip_prefix('_'))
                == Some(&*unique.index)
        }
        None => index.ends_with(&format!("_{}", unique.index)),
    };
    let exact = unique_fields.iter().find(|unique| index == unique.index);
    match exact.or_else(|| unique_fields.iter().find(prefixed)) {
        Some(unique) => Error::Invalid(vec![FieldError::new(
            unique.field,
            UNIQUE_KEY,
            unique.message,
        )]),
        None => error,
    }
}

pub trait Outcome: Send {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        let _ = plan;
        Ok(())
    }

    fn committed(&mut self, stamp: Stamp) {
        let _ = stamp;
    }

    /// What was planned is already committed when serializing fails.
    fn reply(self) -> Result<Reply>
    where
        Self: Sized,
    {
        Ok(Reply::Empty)
    }
}

/// What a GET may answer with: its outcome is replied, never committed.
#[diagnostic::on_unimplemented(
    message = "a GET, screen or island handler cannot return `{Self}`: a GET never writes",
    label = "a writable outcome",
    note = "return a read model, `Paginated`, `Screen` or another reply; a write belongs in a POST, or in a job a domain event triggers"
)]
pub trait ReadOnly: Outcome {}

impl Outcome for () {}

impl ReadOnly for () {}

impl<T: ReadOnly> ReadOnly for Vec<T> {}

impl<T: Outcome> Outcome for Vec<T> {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.iter().try_for_each(|outcome| outcome.plan(plan))
    }

    fn committed(&mut self, stamp: Stamp) {
        for outcome in self {
            outcome.committed(stamp);
        }
    }

    fn reply(self) -> Result<Reply> {
        self.into_iter()
            .map(Outcome::reply)
            .collect::<Result<_>>()
            .map(Reply::List)
    }
}

impl<T: Outcome> Outcome for Option<T> {
    fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
        self.as_ref().map_or(Ok(()), |outcome| outcome.plan(plan))
    }

    fn committed(&mut self, stamp: Stamp) {
        if let Some(outcome) = self {
            outcome.committed(stamp);
        }
    }

    fn reply(self) -> Result<Reply> {
        self.map_or(Ok(Reply::Empty), Outcome::reply)
    }
}

macro_rules! tuple_outcome {
    ($($name:ident),+) => {
        #[allow(non_snake_case)]
        impl<$($name: Outcome),+> Outcome for ($($name,)+) {
            fn plan(&self, plan: &mut Plan<'_>) -> Result<()> {
                let ($($name,)+) = self;
                $($name.plan(plan)?;)+
                Ok(())
            }

            fn committed(&mut self, stamp: Stamp) {
                let ($($name,)+) = self;
                $($name.committed(stamp);)+
            }

            /// The first part answering anything: `(Booking, Queued)` answers the booking.
            fn reply(self) -> Result<Reply> {
                let ($($name,)+) = self;
                $(
                    let reply = $name.reply()?;
                    if !matches!(reply, Reply::Empty) {
                        return Ok(reply);
                    }
                )+
                Ok(Reply::Empty)
            }
        }

        impl<$($name: ReadOnly),+> ReadOnly for ($($name,)+) {}
    };
}

tuple_outcome!(A, B);
tuple_outcome!(A, B, C);
tuple_outcome!(A, B, C, D);

#[cfg(test)]
mod tests {
    use super::*;

    fn email_field() -> Vec<UniqueField> {
        vec![UniqueField {
            index: Cow::Borrowed("account_email"),
            module: Some("shop"),
            field: "email",
            message: "is already registered",
        }]
    }

    fn violation(index: &str) -> Error {
        Error::internal(
            "running an insert",
            UniqueViolation {
                index: index.to_owned(),
            },
        )
    }

    #[test]
    fn an_absent_outcome_replies_empty_and_a_present_one_replies_its_own() {
        let absent: Option<Vec<()>> = None;

        assert!(matches!(absent.reply().unwrap(), Reply::Empty));
        assert!(matches!(
            Some(vec![()]).reply().unwrap(),
            Reply::List(items) if matches!(items[..], [Reply::Empty])
        ));
    }

    struct Saved;

    impl Outcome for Saved {
        fn reply(self) -> Result<Reply> {
            Ok(Reply::Json(serde_json::json!({ "id": "1" })))
        }
    }

    #[test]
    fn a_tuple_replies_its_first_part_that_answers_anything() {
        let reply = ((), Saved, Saved).reply().unwrap();

        assert!(
            matches!(&reply, Reply::Json(value) if value["id"] == "1"),
            "{:?}",
            reply.to_json()
        );
        assert!(matches!(((), ()).reply().unwrap(), Reply::Empty));
    }

    #[test]
    fn a_violation_named_after_the_module_s_table_is_invalid_on_its_field() {
        let error = explain_duplicate(violation("shop_account_email"), &email_field());

        assert!(matches!(&error, Error::Invalid(fields) if fields[0].field == "email"));
    }

    #[test]
    fn a_violation_on_a_table_whose_name_ends_like_another_s_is_not_put_on_its_field() {
        let error = explain_duplicate(violation("billing_account_email_key"), &email_field());

        assert!(matches!(error, Error::Internal { .. }), "{error}");
    }

    #[test]
    fn a_violation_of_a_named_index_is_invalid_on_its_field() {
        let error = explain_duplicate(violation("account_email"), &email_field());

        assert!(
            matches!(&error, Error::Invalid(fields) if fields[0].field == "email"
                && fields[0].key == "validation.unique"
                && fields[0].message == "is already registered"),
            "{error}"
        );
    }

    #[test]
    fn any_other_failure_is_left_as_it_was() {
        let unnamed = explain_duplicate(violation("other_index"), &email_field());
        let plain = explain_duplicate(Error::internal_message("lost"), &email_field());

        assert!(matches!(unnamed, Error::Internal { .. }), "{unnamed}");
        assert!(matches!(plain, Error::Internal { .. }), "{plain}");
    }
}
