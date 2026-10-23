//! A `tracing` field's name is fixed where written, so a value's members reach the subscriber
//! beside its event, through the logging thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::RwLock;

use sofie_core::unpoisoned;
use sofie_values::{LogFields, LogValue, SensitiveFields};

use crate::fields::{FieldValue, Fields};
use crate::scrub::MASK;

thread_local! {
    static PENDING: RefCell<Option<Fields>> = const { RefCell::new(None) };
}

static JOINED: RwLock<Option<HashMap<(&str, &str), &'static str>>> = RwLock::new(None);

#[doc(hidden)]
#[derive(Default)]
pub struct Logged(Fields);

impl Logged {
    #[must_use]
    pub fn with(mut self, name: &'static str, value: &(impl LogFields + ?Sized)) -> Self {
        value.log_fields(&mut |member, logged| {
            self.0.set(joined(name, member), field_value(logged));
        });
        self
    }

    fn masked(mut self, name: &'static str) -> Self {
        self.0.set(name, FieldValue::Text(MASK.to_owned()));
        self
    }

    pub fn emit(self, event: impl FnOnce()) {
        let _restored = Restored(PENDING.replace(Some(self.0)));
        event();
    }
}

/// Restores the outer fields even when the event panics, so no later event takes these.
struct Restored(Option<Fields>);

impl Drop for Restored {
    fn drop(&mut self) {
        let _ = PENDING.try_with(|pending| pending.replace(self.0.take()));
    }
}

/// `order.tax_number` logged by its path: the field's type says nothing of `#[sensitive]`, its
/// owner's `SensitiveFields` does. Which trait answers is chosen by autoref (`(&&member)`), so a
/// type without the trait logs the field as its type does.
#[doc(hidden)]
pub struct Member<'a, Owner: ?Sized, Value: ?Sized> {
    owner: PhantomData<&'a Owner>,
    field: &'static str,
    value: &'a Value,
}

impl<'a, Owner: ?Sized, Value: ?Sized> Member<'a, Owner, Value> {
    pub fn new(_owner: &'a Owner, field: &'static str, value: &'a Value) -> Self {
        Self {
            owner: PhantomData,
            field,
            value,
        }
    }
}

#[doc(hidden)]
pub trait SensitiveMember {
    fn log_into(&self, logged: Logged) -> Logged;
}

impl<Owner: SensitiveFields + ?Sized, Value: LogFields + ?Sized> SensitiveMember
    for &Member<'_, Owner, Value>
{
    fn log_into(&self, logged: Logged) -> Logged {
        if Owner::SENSITIVE_FIELDS.contains(&self.field) {
            return logged.masked(self.field);
        }
        logged.with(self.field, self.value)
    }
}

#[doc(hidden)]
pub trait PlainMember {
    fn log_into(&self, logged: Logged) -> Logged;
}

impl<Owner: ?Sized, Value: LogFields + ?Sized> PlainMember for Member<'_, Owner, Value> {
    fn log_into(&self, logged: Logged) -> Logged {
        logged.with(self.field, self.value)
    }
}

pub(crate) fn take() -> Option<Fields> {
    PENDING
        .try_with(|pending| pending.borrow_mut().take())
        .ok()
        .flatten()
}

/// Both halves are `'static` names from code, so the joined names are few and kept for good.
fn joined(name: &'static str, member: Option<&'static str>) -> &'static str {
    let Some(member) = member else {
        return name;
    };
    let known = unpoisoned::read(&JOINED)
        .as_ref()
        .and_then(|names| names.get(&(name, member)).copied());
    if let Some(known) = known {
        return known;
    }
    let mut names = unpoisoned::write(&JOINED);
    names
        .get_or_insert_default()
        .entry((name, member))
        .or_insert_with(|| Box::leak(format!("{name}.{member}").into_boxed_str()))
}

fn field_value(logged: LogValue<'_>) -> FieldValue {
    match logged {
        LogValue::Integer(number) => FieldValue::Signed(number),
        LogValue::Float(number) => FieldValue::Float(number),
        LogValue::Bool(value) => FieldValue::Bool(value),
        LogValue::Text(text) => FieldValue::Text(text.into_owned()),
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! __with {
    ($logged:ident; $value:ident) => {
        $logged.with(stringify!($value), &$value)
    };
    ($logged:ident; $owner:ident $($rest:ident)+) => {
        $crate::__with!(@ $logged; ($owner) $($rest)+)
    };
    (@ $logged:ident; ($($owner:tt)+) $field:ident) => {{
        #[allow(unused_imports)]
        use $crate::{PlainMember as _, SensitiveMember as _};
        (&&$crate::Member::new(&$($owner)+, stringify!($field), &$($owner)+.$field))
            .log_into($logged)
    }};
    (@ $logged:ident; ($($owner:tt)+) $next:ident $($rest:ident)+) => {
        $crate::__with!(@ $logged; ($($owner)+ . $next) $($rest)+)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __gather {
    ([$( ($head:ident $($rest:ident)*) )*]) => {{
        let logged = $crate::Logged::default();
        $( let logged = $crate::__with!(logged; $head $($rest)*); )*
        logged
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __log {
    ($level:ident [$($fields:tt)*] $message:literal) => {
        if $crate::__tracing::enabled!($crate::__tracing::Level::$level) {
            $crate::__gather!([$($fields)*]).emit(|| {
                $crate::__tracing::event!($crate::__tracing::Level::$level, $message);
            });
        }
    };
    ($level:ident [$($fields:tt)*] $head:ident $(. $rest:ident)* , $($tail:tt)+) => {
        $crate::__log!($level [$($fields)* ($head $($rest)*)] $($tail)+)
    };
}

#[macro_export]
macro_rules! error {
    ($($arguments:tt)+) => { $crate::__log!(ERROR [] $($arguments)+) };
}

#[macro_export]
macro_rules! warn {
    ($($arguments:tt)+) => { $crate::__log!(WARN [] $($arguments)+) };
}

/// The message is a literal, so no value is formatted into it; sensitive values log their mask.
#[macro_export]
macro_rules! info {
    ($($arguments:tt)+) => { $crate::__log!(INFO [] $($arguments)+) };
}

#[macro_export]
macro_rules! debug {
    ($($arguments:tt)+) => { $crate::__log!(DEBUG [] $($arguments)+) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_that_panics_leaves_no_fields_for_the_next_one() {
        let mut fields = Fields::default();
        fields.set("order.total", FieldValue::Signed(12));

        let panicked =
            std::panic::catch_unwind(|| Logged(fields).emit(|| panic!("a Debug impl panicked")));

        assert!(panicked.is_err());
        assert!(take().is_none());
    }
}
