use std::marker::PhantomData;

use http::request::Parts;
use sofie_core::FieldError;

use crate::Screens;

/// Without `i18n` everything is written in English, the language of the app's names.
#[derive(Clone, Copy)]
pub(crate) struct Language<'a>(PhantomData<&'a ()>);

#[expect(
    clippy::unused_self,
    reason = "the same calls reach the translations under the `i18n` feature"
)]
impl Language<'_> {
    pub(crate) fn plain() -> Language<'static> {
        Language(PhantomData)
    }

    pub(crate) fn tag(self) -> &'static str {
        "en"
    }

    pub(crate) fn varies_by(self) -> &'static str {
        crate::screen::VARIES_BY
    }

    pub(crate) fn field_error(self, error: &FieldError) -> String {
        error.message.clone()
    }

    pub(crate) fn label(self, field: &str) -> String {
        sofie_engine::humanize(field)
    }

    pub(crate) fn script_notices(self) -> sofie_engine::ScriptNotices<'static> {
        sofie_engine::ScriptNotices::plain()
    }

    pub(crate) fn fields_notice(self, count: usize) -> String {
        let summary = match count {
            1 => "Please correct the highlighted field.".to_owned(),
            count => format!("Please correct the {count} highlighted fields."),
        };
        sofie_engine::notice(&[summary])
    }
}

impl Screens {
    #[expect(
        clippy::unused_self,
        reason = "under the `i18n` feature the screens hold the translations"
    )]
    pub(crate) fn language(&self, _request: &Parts) -> Language<'_> {
        Language::plain()
    }
}
