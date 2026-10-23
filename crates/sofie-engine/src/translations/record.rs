use super::{Call, CountedKeys, Language};
use crate::message::Recorded;
use crate::parse::each_call;
use crate::syntax::{Op, Written};

/// Each `t(…)` call keeps its text in the source language, which a build without `i18n` writes.
pub(crate) fn record(ops: &mut [Op<Written>], source: Language<'_>) {
    each_call(ops, &mut |call| {
        if let Call::Translate {
            key,
            counted,
            recorded,
            ..
        } = call
        {
            *recorded = source_text(source, key, counted.as_ref());
        }
    });
}

fn source_text(source: Language<'_>, key: &str, counted: Option<&CountedKeys>) -> Option<Recorded> {
    let Some(keys) = counted else {
        return Some(Recorded {
            other: source.message(key)?.clone(),
            one: None,
        });
    };
    let other = source
        .message(&keys.other)
        .or_else(|| source.message(key))?;
    Some(Recorded {
        other: other.clone(),
        one: source.own_message(&keys.one).cloned(),
    })
}
