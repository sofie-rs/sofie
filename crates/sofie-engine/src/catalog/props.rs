pub use sofie_core::is_field_name;

use super::spec::ComponentSpec;
use crate::render::parts_of;
use crate::syntax::{Attribute, Expression, LocatedError, Op, Position};

pub(super) struct Props {
    spec: &'static ComponentSpec,
    attributes: Vec<Attribute>,
    position: Position,
}

impl Props {
    pub(super) fn new(
        spec: &'static ComponentSpec,
        attributes: Vec<Attribute>,
        position: Position,
    ) -> Self {
        Self {
            spec,
            attributes,
            position,
        }
    }

    pub(super) fn take(&mut self, name: &str) -> Option<Attribute> {
        self.check_declared(name, false);
        self.take_declared(name)
    }

    /// A prop read here but missing from the spec, or read as required against it, is the
    /// engine's own defect, so every test that builds the component finds it.
    fn check_declared(&self, name: &str, required: bool) {
        debug_assert!(
            self.spec
                .props
                .iter()
                .any(|prop| prop.name == name && prop.required == required),
            "`{}` reads `{name}` (required: {required}) unlike its spec",
            self.spec.name
        );
    }

    fn take_declared(&mut self, name: &str) -> Option<Attribute> {
        let index = self
            .attributes
            .iter()
            .position(|attribute| attribute.name == name)?;
        Some(self.attributes.remove(index))
    }

    /// `name={{field: value, …}}`, which the parser wrote as one attribute `name.field` per
    /// field; each is given back named by its field.
    pub(super) fn fields(&mut self, name: &str) -> Vec<Attribute> {
        self.check_declared(name, false);
        let prefix = format!("{name}.");
        self.attributes
            .extract_if(.., |attribute| attribute.name.starts_with(&prefix))
            .map(|mut attribute| {
                attribute.name.drain(..prefix.len());
                attribute
            })
            .collect()
    }

    pub(super) fn optional(&mut self, name: &str) -> Option<Expression> {
        self.take(name).map(|attribute| attribute.value)
    }

    pub(super) fn required(&mut self, name: &str) -> Result<Expression, LocatedError> {
        self.required_attribute(name)
            .map(|attribute| attribute.value)
    }

    /// Read as one value, so `{…}` parts inside quotes would have no meaning.
    #[cfg(feature = "islands")]
    pub(super) fn required_value(&mut self, name: &str) -> Result<Expression, LocatedError> {
        let attribute = self.required_attribute(name)?;
        self.single(attribute)
    }

    /// Data only: a literal or `{"…"}` here would render nothing on every request.
    pub(super) fn required_path(&mut self, name: &str) -> Result<Expression, LocatedError> {
        let attribute = self.required_attribute(name)?;
        check_path(self.spec.name, &attribute)?;
        Ok(attribute.value)
    }

    pub(super) fn optional_value(
        &mut self,
        name: &str,
    ) -> Result<Option<Expression>, LocatedError> {
        self.take(name)
            .map(|attribute| self.single(attribute))
            .transpose()
    }

    /// The browser encodes or decodes some characters in a literal URL, so it would post to
    /// another URL than the written one and a refused form could not show its page again.
    pub(super) fn required_post(&mut self) -> Result<Expression, LocatedError> {
        let attribute = self.required_attribute("post")?;
        let unsafe_character = parts_of(&attribute.value)
            .iter()
            .find_map(|part| match part {
                Expression::Authored(text) => text.chars().find(|&c| !is_literal_url_character(c)),
                _ => None,
            });
        if let Some(character) = unsafe_character {
            bail!(
                attribute.position,
                "`post` on `{}` has {character:?}, which a browser would change; percent-encode it",
                self.spec.name
            );
        }
        Ok(attribute.value)
    }

    pub(super) fn required_literal(
        &mut self,
        name: &str,
    ) -> Result<(String, Position), LocatedError> {
        let attribute = self.required_attribute(name)?;
        match attribute.value {
            Expression::Authored(text) => Ok((text, attribute.position)),
            _ => bail!(
                attribute.position,
                "`{}` needs `{name}` as a quoted literal without `{{…}}`",
                self.spec.name
            ),
        }
    }

    pub(super) fn flag(&mut self, name: &str) -> Result<bool, LocatedError> {
        let Some(attribute) = self.take(name) else {
            return Ok(false);
        };
        match attribute.value {
            Expression::Authored(text) if text.is_empty() => Ok(true),
            _ => bail!(
                attribute.position,
                "`{name}` on `{}` takes no value; write it bare",
                self.spec.name
            ),
        }
    }

    pub(super) fn refuse_children(&self, children: &[Op]) -> Result<(), LocatedError> {
        if children.is_empty() {
            return Ok(());
        }
        bail!(
            self.position,
            "`{}` takes no children; write it as `<{} … />`",
            self.spec.name,
            self.spec.name
        );
    }

    pub(super) fn finish(self) -> Result<(), LocatedError> {
        let Some(unknown) = self.attributes.first() else {
            return Ok(());
        };
        let written = unknown
            .name
            .split_once('.')
            .map_or(unknown.name.as_str(), |(written, _)| written);
        bail!(
            unknown.position,
            "`{}` has no prop `{written}`",
            self.spec.name
        );
    }

    fn required_attribute(&mut self, name: &str) -> Result<Attribute, LocatedError> {
        self.check_declared(name, true);
        let position = self.position;
        let component = self.spec.name;
        self.take_declared(name).ok_or_else(|| {
            LocatedError::new(position, format!("`{component}` needs the `{name}` prop"))
        })
    }

    fn single(&self, attribute: Attribute) -> Result<Expression, LocatedError> {
        check_single(self.spec.name, &attribute)?;
        Ok(attribute.value)
    }
}

pub(super) fn check_single(component: &str, attribute: &Attribute) -> Result<(), LocatedError> {
    if !matches!(attribute.value, Expression::Interpolated(_)) {
        return Ok(());
    }
    bail!(
        attribute.position,
        "`{}` on `{component}` is one value: write `{{…}}` or quoted text, not text with `{{…}}` parts",
        attribute.name
    );
}

pub(super) fn check_path(component: &str, attribute: &Attribute) -> Result<(), LocatedError> {
    check_single(component, attribute)?;
    if matches!(attribute.value, Expression::Path(_)) {
        return Ok(());
    }
    bail!(
        attribute.position,
        "`{}` on `{component}` reads data: bind it to a path such as `{{data.{}}}`",
        attribute.name,
        attribute.name
    );
}

/// Only plain ids, so an id can never become a broader CSS selector. A leading digit would make
/// `#id` no selector at all, so htmx would find no target.
#[must_use]
pub(crate) fn is_element_id(text: &str) -> bool {
    text.starts_with(|first: char| first.is_ascii_alphabetic())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// RFC 3986's unreserved and reserved characters and `%`, less `&`, which HTML may decode.
fn is_literal_url_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || "-._~!$'()*+,;=:@/?#%".contains(character)
}

#[cfg(test)]
mod tests {
    use super::is_element_id;

    #[test]
    fn an_element_id_starts_with_a_letter() {
        assert!(is_element_id("orders-2_a"));
        for refused in ["", "1", "1orders", "-a", "_a", "a.b", "a b", "a#b"] {
            assert!(!is_element_id(refused), "{refused}");
        }
    }
}
