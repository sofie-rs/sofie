#[cfg(feature = "gates")]
mod gates;
#[cfg(all(feature = "gates", feature = "parse"))]
mod parse;
#[cfg(feature = "gates")]
mod withheld;

use sofie_core::Show;

#[cfg(feature = "gates")]
pub use self::gates::Gates;
#[cfg(feature = "gates")]
pub(crate) use self::gates::{Asks, Gate, GatesRef};
#[cfg(all(feature = "gates", feature = "parse"))]
pub(crate) use self::parse::parse_gate;
#[cfg(feature = "gates")]
pub(crate) use self::withheld::GatedPaths;
#[cfg(feature = "gates")]
pub use self::withheld::Withheld;
#[cfg(all(feature = "gates", feature = "analysis"))]
pub use self::withheld::{Withholdable, withholdable};
use crate::language::Language;
use crate::template::{mark_out_of_band, render_part, render_whole};
use crate::{RenderError, Submitted, Template, TemplateError};

/// [`Template`]'s own renders are for no request: plain language, nothing gated shown.
#[derive(Clone, Copy)]
pub struct Rendering<'t> {
    template: &'t Template,
    language: Language<'t>,
}

impl<'t> Rendering<'t> {
    #[must_use]
    pub fn new(template: &'t Template) -> Self {
        Self {
            template,
            language: Language::plain(),
        }
    }

    #[cfg(feature = "i18n")]
    #[must_use]
    pub fn language(mut self, language: crate::Language<'t>) -> Self {
        #[cfg(feature = "gates")]
        let language = language.with_gates(self.language.gates());
        self.language = language;
        self
    }

    #[cfg(feature = "gates")]
    #[must_use]
    pub fn gates(mut self, gates: &'t dyn Gates) -> Self {
        self.language = self.language.with_gates(Some(gates));
        self
    }

    /// The data paths bound only inside parts this viewer may not see, which JSON and a client
    /// area's envelope leave out as the page does.
    #[cfg(feature = "gates")]
    #[must_use]
    pub fn withheld(&self) -> Withheld<'t> {
        self.template
            .precomputed
            .gated_paths
            .withheld(self.language.gates())
    }

    /// What a client area's screen is given beside its data, in this rendering's language and
    /// for its viewer.
    #[cfg(feature = "clients")]
    #[must_use]
    pub fn client_view(&self) -> serde_json::Value {
        crate::client_view::client_parts(self.template).view(self.language)
    }

    /// A listing's refused filters in a client area's data, written in this rendering's language.
    #[cfg(all(feature = "clients", feature = "listing"))]
    pub fn translate_listing_errors(&self, data: &mut serde_json::Value) {
        crate::client_view::client_parts(self.template)
            .translate_listing_errors(data, self.language);
    }

    pub fn render(&self, data: &(impl Show + ?Sized)) -> Result<String, TemplateError> {
        render_whole(self.template, data.show(), None, self.language)
    }

    pub fn render_submitted(
        &self,
        data: &(impl Show + ?Sized),
        submitted: &Submitted<'_>,
    ) -> Result<String, TemplateError> {
        render_whole(self.template, data.show(), Some(submitted), self.language)
    }

    pub fn render_fragment(
        &self,
        data: &(impl Show + ?Sized),
        id: &str,
    ) -> Result<String, RenderError> {
        render_part(self.template, data.show(), id, self.language)
    }

    pub fn render_out_of_band(
        &self,
        data: &(impl Show + ?Sized),
        id: &str,
    ) -> Result<String, RenderError> {
        self.render_fragment(data, id).map(mark_out_of_band)
    }
}
