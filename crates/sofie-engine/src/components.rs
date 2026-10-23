use std::collections::BTreeMap;

use crate::catalog::Kind;
use crate::error::TemplateError;

/// A module's `web/<Name>.html`, written in a screen as `<Name prop={…} />` and
/// inlined where it is written, so rendering pays nothing for it.
#[derive(Clone, Debug, Default)]
pub struct Components {
    by_name: BTreeMap<String, String>,
}

impl Components {
    /// Fails for a name the catalog already has, one given twice, or one that is not `PascalCase`.
    pub fn add(&mut self, name: &str, source: String) -> Result<(), TemplateError> {
        let refused = |message: String| TemplateError::unlocated(name, message);
        if !is_component_name(name) {
            return Err(refused(format!(
                "`{name}` is no component name: name `web/<Name>.html` in PascalCase, such as \
                 `OrderCard`"
            )));
        }
        if Kind::named(name).is_some() {
            return Err(refused(format!(
                "`{name}` is a component the catalog ships; name the template otherwise"
            )));
        }
        if self.by_name.insert(name.to_owned(), source).is_some() {
            return Err(refused(format!(
                "component `{name}` is added twice; one `web/{name}.html` per app"
            )));
        }
        Ok(())
    }

    pub(crate) fn source(&self, name: &str) -> Option<&str> {
        self.by_name.get(name).map(String::as_str)
    }
}

fn is_component_name(name: &str) -> bool {
    name.starts_with(|first: char| first.is_ascii_uppercase())
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric())
}
