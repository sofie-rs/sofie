//! There is no way to emit raw HTML from data.

macro_rules! bail {
    ($position:expr, $($message:tt)+) => {
        return Err($crate::syntax::LocatedError::new($position, format!($($message)+)))
    };
}

#[cfg(feature = "analysis")]
mod analysis;
mod asset;
mod catalog;
#[cfg(feature = "client-compile")]
mod client;
#[cfg(feature = "clients")]
mod client_instants;
#[cfg(feature = "clients")]
mod client_view;
#[cfg(feature = "clients")]
mod clients;
mod compiled;
#[cfg(feature = "parse")]
mod components;
#[cfg(feature = "development")]
pub mod development;
mod error;
mod escape;
#[cfg(feature = "islands")]
pub mod islands;
mod label;
#[cfg(not(feature = "i18n"))]
mod language;
mod message;
mod page;
#[cfg(feature = "parse")]
mod parse;
mod program;
mod render;
#[cfg(any(feature = "i18n", feature = "gates"))]
pub mod rendering;
#[cfg(any(feature = "analysis", feature = "gates"))]
mod slots;
mod syntax;
mod template;
#[cfg(feature = "i18n")]
mod translations;
#[cfg(any(feature = "clients", feature = "gates"))]
mod walk;
#[cfg(feature = "i18n")]
use translations as language;

#[cfg(feature = "analysis")]
pub use crate::analysis::{
    ActionPost, Binding, FormPost, IdPattern, Step, TableLink, action_posts_with, bindings,
    bindings_with, table_links_with,
};
#[cfg(all(feature = "analysis", feature = "gates"))]
pub use crate::analysis::{AskedEntitlement, asked_entitlements};
#[cfg(feature = "compression")]
pub use crate::asset::Precompressed;
pub use crate::asset::{Asset, CLIENT_SCRIPT, ENGINE_SCRIPT, STYLESHEET};
pub use crate::catalog::{
    Children, ComponentSpec, PropKind, PropSpec, SPECS, Submitted, field_errors, is_field_name,
};
#[cfg(feature = "client-compile")]
pub use crate::client::{Client, compile_client};
#[cfg(feature = "clients")]
pub use crate::client_instants::instant_fields;
#[cfg(feature = "clients")]
pub use crate::client_view::{ClientView, plain_client_view};
#[cfg(feature = "clients")]
pub use crate::clients::{ClientBundle, ClientLinks, ClientShell, SCREEN_ELEMENT};
#[cfg(all(feature = "parse", feature = "i18n"))]
pub use crate::compiled::compile_template_with_texts;
pub use crate::compiled::load_template;
#[cfg(feature = "parse")]
pub use crate::compiled::{compile_template, compile_template_with, parse_template_with_recorded};
#[cfg(feature = "parse")]
pub use crate::components::Components;
pub use crate::error::{RenderError, TemplateError};
pub use crate::escape::push_escaped;
pub use crate::label::humanize;
pub use crate::page::{NOTICE_TARGET, Page, ScriptNotices, notice, with_cleared_notice};
pub use crate::template::{Template, form_fields};
#[cfg(feature = "i18n")]
pub use crate::translations::{CURRENCY_SYMBOLS, Language, Tone, TranslationError, Translations};
#[cfg(all(feature = "i18n", feature = "parse"))]
pub use crate::translations::{
    Gap, GapKind, KeyKind, KeyUse, check_translations, translation_keys,
};
