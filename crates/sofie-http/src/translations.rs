use http::header::ACCEPT_LANGUAGE;
use http::request::Parts;
use sofie_core::{Error, FieldError, Result};
use sofie_db::File;
use sofie_engine::rendering::Rendering;
use sofie_engine::{Template, Tone, TranslationError, Translations};

use crate::Screens;

/// As `sofie gen` writes them: the source language first, then each module's
/// `lang/<language>.toml` files; `enums` by their `enum.<name>.<variant>` names, so badges
/// and selects show their variants translated.
pub fn translations(
    languages: &[&str],
    files: &[(&str, File)],
    builtin: &[(&str, &[File])],
    enums: &[(&str, &[(&str, Tone)])],
) -> Result<Translations> {
    let refused =
        |error: TranslationError| Error::internal("loading the app's translations", error);
    let mut translations = Translations::new(languages).map_err(refused)?;
    for (module, file) in files {
        translations
            .add(module, file.stem(), file.text()?)
            .map_err(refused)?;
    }
    for (module, files) in builtin {
        let sources = files
            .iter()
            .map(|file| Ok((file.stem(), file.text()?)))
            .collect::<Result<Vec<_>>>()?;
        translations
            .add_builtin(module, &sources)
            .map_err(refused)?;
    }
    for (name, variants) in enums {
        translations.add_enum(name, variants).map_err(refused)?;
    }
    Ok(translations)
}

impl Screens {
    /// `sofie check` refuses a screen using a key the source language lacks, as the app is built.
    pub fn translate(&mut self, translations: Translations) {
        self.translations = Some(translations);
    }

    pub(crate) fn language(&self, request: &Parts) -> Language<'_> {
        let accepted = request
            .headers
            .get(ACCEPT_LANGUAGE)
            .and_then(|value| value.to_str().ok());
        match &self.translations {
            Some(translations) => Language {
                language: translations.negotiate(None, accepted),
                negotiated: true,
            },
            None => Language::plain(),
        }
    }
}

/// `negotiated`: a cache must keep answers apart by `Accept-Language`.
#[derive(Clone, Copy)]
pub(crate) struct Language<'a> {
    language: sofie_engine::Language<'a>,
    negotiated: bool,
}

impl<'a> Language<'a> {
    pub(crate) fn plain() -> Language<'static> {
        Language {
            language: sofie_engine::Language::plain(),
            negotiated: false,
        }
    }

    pub(crate) fn varies_by(self) -> &'static str {
        if self.negotiated {
            return "Accept, Accept-Language, HX-Request, HX-Target";
        }
        crate::screen::VARIES_BY
    }

    pub(crate) fn screen<'t>(self, template: &'t Template) -> Rendering<'t>
    where
        'a: 't,
    {
        Rendering::new(template).language(self.language)
    }

    pub(crate) fn tag(self) -> &'a str {
        self.language.locale().as_str()
    }

    pub(crate) fn field_error(self, error: &FieldError) -> String {
        let arguments: Vec<(&str, String)> = error
            .arguments
            .iter()
            .map(|(name, value)| (name.as_ref(), value.clone()))
            .collect();
        self.language
            .field_error(&error.key, &arguments, &error.message)
    }

    pub(crate) fn label(self, field: &str) -> String {
        let key = format!("label.{field}");
        match self.language.field_message(&key) {
            text if text == key => sofie_engine::humanize(field),
            text => text.to_owned(),
        }
    }

    pub(crate) fn script_notices(self) -> sofie_engine::ScriptNotices<'a> {
        sofie_engine::ScriptNotices::new(self.language)
    }

    pub(crate) fn fields_notice(self, count: usize) -> String {
        let summary = match count {
            1 => self.language.field_error("notice.correct_field", &[], ""),
            count => self.language.field_error(
                "notice.correct_fields",
                &[("count", count.to_string())],
                "",
            ),
        };
        sofie_engine::notice(&[summary])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turkish() -> Translations {
        translations(
            &["en", "tr"],
            &[(
                "order",
                File::new("lang/tr.toml", b"[label]\nstatus = \"Durum\"\n"),
            )],
            &[],
            &[],
        )
        .unwrap()
    }

    #[test]
    fn the_fields_notice_counts_in_the_viewer_s_language() {
        let translations = turkish();
        let language = Language {
            language: translations.negotiate(None, Some("tr-TR, en;q=0.5")),
            negotiated: true,
        };

        assert_eq!(language.tag(), "tr");
        assert_eq!(
            language.fields_notice(3),
            "<p>Lütfen işaretli 3 alanı düzeltin.</p>"
        );
        assert_eq!(
            Language::plain().fields_notice(1),
            "<p>Please correct the highlighted field.</p>"
        );
    }

    #[test]
    fn a_label_is_translated_or_humanized() {
        let translations = turkish();
        let language = Language {
            language: translations.negotiate(None, Some("tr")),
            negotiated: true,
        };

        assert_eq!(language.label("status"), "Durum");
        assert_eq!(language.label("placed_at"), "Placed at");
        let error =
            FieldError::new("total", "validation.max", "must be at most 5").with("max", "5");
        assert_eq!(language.field_error(&error), "en fazla 5 olmalı");
    }

    #[test]
    fn a_json_refusal_reads_in_the_viewer_s_language() {
        let translations = turkish();
        let language = Language {
            language: translations.negotiate(None, Some("tr")),
            negotiated: true,
        };
        let refusal = FieldError::new(
            "total.gt",
            "validation.kind.whole_number",
            "must be a whole number",
        );

        let translated = crate::answer::in_language(Err(Error::Invalid(vec![refusal])), language);

        let Err(Error::Invalid(fields)) = translated else {
            panic!("{:?}", translated.err());
        };
        assert_eq!(fields[0].field, "total.gt");
        assert_eq!(fields[0].message, "tam sayı olmalı");
    }

    #[test]
    fn an_undeclared_language_file_is_refused() {
        let error = translations(
            &["en"],
            &[("order", File::new("lang/tr.toml", b""))],
            &[],
            &[],
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("loading the app's translations"),
            "{error}"
        );
    }
}
