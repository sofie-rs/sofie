//! Derives emit trait impls and descriptions only; behaviour lives in the libraries.

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

mod aggregate;
mod case;
#[cfg(feature = "children")]
mod children;
#[cfg(feature = "config")]
mod config;
mod constraints;
#[cfg(feature = "contracts")]
mod contracts;
mod crate_path;
mod enumeration;
mod event;
mod every;
#[cfg(feature = "factories")]
mod factory;
#[cfg(feature = "fixtures")]
mod fixture;
mod input;
#[cfg(feature = "joins")]
mod joins;
#[cfg(feature = "retention")]
mod pruned;
mod read_model;
#[cfg(feature = "remote-contracts")]
mod remote;
mod sensitive;
mod serialize;
mod shape;
#[cfg(feature = "span")]
mod span;
#[cfg(feature = "tasks")]
mod task;
mod value;

/// `#[owner]` marks the `Id<…>` field naming the owner; on the struct, a record that is its
/// own owner, or with `(through = parent)` one owned by its parent's owner.
/// `#[aggregate(behaviors(Slugged(from = name)))]` adds behaviors to `Versioned` and
/// `Timestamps`. Field constraints are those of `#[derive(Input)]`. A `#[sensitive]` field shows
/// as `***` in `Debug` and logs; it and one of a sensitive type stay out of JSON and search.
#[proc_macro_derive(
    Aggregate,
    attributes(
        sofie, owner, aggregate, min, max, length, range, required, unique, expose, mirror,
        sensitive
    )
)]
pub fn derive_aggregate(item: TokenStream) -> TokenStream {
    expand(item, aggregate::derive)
}

/// Needs an `id: Id<Self>` field; stored in its own table beside the aggregate's.
#[cfg(feature = "children")]
#[proc_macro_derive(Child, attributes(sofie, min, max, length, range, required))]
pub fn derive_child(item: TokenStream) -> TokenStream {
    expand(item, children::derive)
}

/// `#[sensitive]` masks it in `Debug`. As a handler's input it is read from the body field named
/// after the type: `cancel(order: Order, cancel_reason: CancelReason)` takes
/// `{ "cancel_reason": … }`, and `Option<CancelReason>` takes it or nothing.
#[proc_macro_derive(Value, attributes(sofie, sensitive))]
pub fn derive_value(item: TokenStream) -> TokenStream {
    expand(item, value::derive)
}

/// Read from its module's `<MODULE>_<FIELD>` keys; a field of another struct deriving it reads
/// `<MODULE>_<FIELD>_*`.
#[cfg(feature = "config")]
#[proc_macro_derive(Config, attributes(sofie))]
pub fn derive_config(item: TokenStream) -> TokenStream {
    expand(item, config::derive)
}

/// `#[to(…)]` lists a variant's transitions.
#[proc_macro_derive(Enum, attributes(sofie, to, tone))]
pub fn derive_enum(item: TokenStream) -> TokenStream {
    expand(item, enumeration::derive)
}

/// `#[read(from = Aggregate)]`; `#[read(via = Base::COLUMN, field = Other::COLUMN)]` reads a
/// column of the record the base's id column names. A field reading a sensitive one needs
/// `#[sensitive]`.
#[proc_macro_derive(ReadModel, attributes(sofie, read, sensitive))]
pub fn derive_read_model(item: TokenStream) -> TokenStream {
    expand(item, read_model::derive)
}

#[proc_macro_derive(Event, attributes(sofie))]
pub fn derive_event(item: TokenStream) -> TokenStream {
    expand(item, event::derive)
}

/// `#[min(n)]`/`#[max(n)]` bound text characters or a number's value; `#[length(min, max)]`,
/// `#[range(min, max)]`, `#[required]` (not blank) and `#[unique]` (held by the database only).
/// `#[input(from = Order)]` inherits `Order`'s field constraints, `#[unconstrained]` drops
/// them; `#[input(validate)]` runs `Validate` after them.
#[proc_macro_derive(
    Input,
    attributes(sofie, input, min, max, length, range, required, unique, unconstrained)
)]
pub fn derive_input(item: TokenStream) -> TokenStream {
    expand(item, input::derive)
}

/// Takes no arguments: naming the event again could only disagree with the argument's type.
#[proc_macro_attribute]
pub fn on(arguments: TokenStream, item: TokenStream) -> TokenStream {
    if arguments.is_empty() {
        return item;
    }
    let error = syn::Error::new_spanned(
        proc_macro2::TokenStream::from(arguments),
        "write a bare `#[on]`: the listener's first argument's type is the event",
    );
    [error.into_compile_error().into(), item]
        .into_iter()
        .collect()
}

/// Its arguments are never recorded.
#[cfg(feature = "span")]
#[proc_macro_attribute]
pub fn span(arguments: TokenStream, item: TokenStream) -> TokenStream {
    span::attribute(&arguments.into(), item.into()).into()
}

/// Read by `sofie gen`, which registers the job with the worker.
#[proc_macro_attribute]
pub fn job(_arguments: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Read by `sofie gen`, which registers the task as `<module>.<function>`.
#[cfg(feature = "tasks")]
#[proc_macro_attribute]
pub fn task(arguments: TokenStream, item: TokenStream) -> TokenStream {
    task::attribute(&arguments.into(), item.into()).into()
}

/// Each `async fn` returns a boxed future so the trait works as `dyn`.
#[cfg(feature = "contracts")]
#[proc_macro_attribute]
pub fn contract(_arguments: TokenStream, item: TokenStream) -> TokenStream {
    contracts::attribute(item.into(), contracts::Boundary::Contract).into()
}

/// Calls run in `<vendor>.<method>` spans; see `sofie::client`.
#[cfg(feature = "gateways")]
#[proc_macro_attribute]
pub fn gateway(_arguments: TokenStream, item: TokenStream) -> TokenStream {
    contracts::attribute(item.into(), contracts::Boundary::Gateway).into()
}

/// Interval as `30s`, `15m`, `1h`, `1d`; `each = query` runs it per listed record, one job each.
/// Or a time on a zone's clock: `daily_at = "03:00", zone = "Europe/Istanbul"`, and with
/// `weekly_on = "monday"` on that day only.
///
/// ```
/// # async fn late() {}
/// #[sofie_macros::every("1h", each = late)]
/// pub async fn remind() {}
/// ```
///
/// ```compile_fail
/// #[sofie_macros::every("1h", each = no_such_query)]
/// pub async fn remind() {}
/// ```
#[proc_macro_attribute]
pub fn every(arguments: TokenStream, item: TokenStream) -> TokenStream {
    every::attribute(arguments.into(), item.into()).into()
}

/// Written `#[sofie::feature(FLAG)]`: a bare `feature` is Rust's own crate attribute.
#[proc_macro_attribute]
pub fn feature(_arguments: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// One database per test; the crate's wiring must export `MODULES`, `app` and `worker`.
#[cfg(feature = "fixtures")]
#[proc_macro_attribute]
pub fn test_app(arguments: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as syn::ItemFn);
    fixture::test_app(arguments.into(), &function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(
    item: TokenStream,
    derive: fn(&DeriveInput) -> syn::Result<proc_macro2::TokenStream>,
) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    derive(&input)
        .and_then(|expanded| crate_path::with_crate_path(&input, expanded))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
