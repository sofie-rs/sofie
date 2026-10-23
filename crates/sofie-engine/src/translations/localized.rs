use super::check::check_use;
use super::{KeyUse, Translations, key_uses};
use crate::{Template, TemplateError};

#[must_use]
pub fn translation_keys(template: &Template) -> Vec<KeyUse> {
    key_uses(&template.ops).expect("the template's calls were checked when it was parsed")
}

pub fn check_translations(
    template: &Template,
    translations: &Translations,
) -> Result<(), TemplateError> {
    for key_use in translation_keys(template) {
        check_use(translations, &key_use).map_err(|message| TemplateError {
            template: template.name.clone(),
            line: key_use.line,
            column: key_use.column,
            message,
        })?;
    }
    Ok(())
}
