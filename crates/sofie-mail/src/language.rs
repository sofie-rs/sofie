use sofie_core::Show;

use crate::Mailer;

#[cfg(feature = "i18n")]
pub(crate) use spoken::{Language, Spoken, rendered};

#[cfg(feature = "i18n")]
pub use spoken::translations;

#[cfg(not(feature = "i18n"))]
pub(crate) use plain::{Language, rendered};

#[cfg(feature = "i18n")]
mod spoken {
    use std::sync::Arc;

    use sofie_core::Show;
    use sofie_db::Db;
    use sofie_engine::rendering::Rendering;
    use sofie_engine::{Template, TemplateError, Translations};

    pub(crate) type Language<'a> = sofie_engine::Language<'a>;

    pub(crate) struct Spoken(pub(crate) Arc<Translations>);

    /// The app's translations, which a mail is written in once its handler names the language.
    pub fn translations(db: &Db, translations: Arc<Translations>) {
        sofie_db::extend(db, Arc::new(Spoken(translations)));
    }

    pub(crate) fn rendered(
        template: &Template,
        data: &(impl Show + ?Sized),
        language: Language<'_>,
    ) -> Result<String, TemplateError> {
        Rendering::new(template).language(language).render(data)
    }
}

#[cfg(not(feature = "i18n"))]
mod plain {
    use std::marker::PhantomData;

    use sofie_core::Show;
    use sofie_engine::{Template, TemplateError};

    /// Without `i18n` a mail is written in the texts its template was compiled with.
    #[derive(Clone, Copy)]
    pub(crate) struct Language<'a>(PhantomData<&'a ()>);

    impl Language<'_> {
        pub(crate) fn plain() -> Self {
            Self(PhantomData)
        }
    }

    pub(crate) fn rendered(
        template: &Template,
        data: &(impl Show + ?Sized),
        _language: Language<'_>,
    ) -> Result<String, TemplateError> {
        template.render(data)
    }
}

impl Mailer {
    /// The recipient's language for the bodies and texts that follow; `None`, or a language the
    /// app does not declare, writes its source language.
    #[cfg(feature = "i18n")]
    #[must_use]
    pub fn in_language(mut self, locale: Option<sofie_values::Locale>) -> Self {
        self.locale = locale;
        self
    }

    /// A subject or another text outside a template; `fallback`, in the source language, where
    /// the app has no translations or none of its languages has the key.
    #[must_use]
    pub fn text(&self, key: &str, arguments: &[(&str, String)], fallback: &str) -> String {
        #[cfg(feature = "i18n")]
        if let Some(text) = self.language().translated(key, arguments) {
            return text;
        }
        #[cfg(not(feature = "i18n"))]
        let _ = (key, arguments);
        fallback.to_owned()
    }

    #[cfg(feature = "i18n")]
    pub(crate) fn language(&self) -> Language<'_> {
        match &self.translations {
            Some(translations) => translations.negotiate(self.locale.as_ref(), None),
            None => Language::plain(),
        }
    }

    #[cfg(not(feature = "i18n"))]
    #[expect(
        clippy::unused_self,
        reason = "under the `i18n` feature the mailer holds the translations"
    )]
    pub(crate) fn language(&self) -> Language<'_> {
        Language::plain()
    }

    pub(crate) fn render(
        &self,
        template: &crate::Template,
        data: &(impl Show + ?Sized),
    ) -> sofie_core::Result<crate::Body> {
        template.render(data, self.language())
    }
}
