use std::sync::atomic::{AtomicUsize, Ordering};

use sofie_core::{Show, Shown};
use sofie_values::FormField;

use crate::catalog::{self, Submitted};
use crate::error::{RenderError, TemplateError};
use crate::language::Language;
use crate::page::CLEARED_NOTICE;
use crate::render;
use crate::syntax::{LocatedError, Op};

/// A rare huge render, a long listing say, must not make every later render reserve as much.
const MAXIMUM_CAPACITY_HINT: usize = 256 * 1024;

#[derive(Debug)]
pub struct Template {
    pub(crate) name: String,
    pub(crate) ops: Vec<Op>,
    /// The last render's length, so the next one allocates once.
    document_capacity: AtomicUsize,
    fragment_capacity: AtomicUsize,
    pub(crate) precomputed: Precomputed,
}

/// What a template's ops imply beside rendering them, worked out once.
#[derive(Debug)]
pub(crate) struct Precomputed {
    #[cfg(feature = "clients")]
    pub(crate) client_parts: std::sync::OnceLock<crate::client_view::ClientParts>,
    #[cfg(feature = "gates")]
    pub(crate) gated_paths: crate::rendering::GatedPaths,
}

impl Precomputed {
    #[cfg_attr(
        not(feature = "gates"),
        expect(unused_variables, reason = "only gates are read from the ops up front")
    )]
    fn of(ops: &[Op]) -> Self {
        Self {
            #[cfg(feature = "clients")]
            client_parts: std::sync::OnceLock::new(),
            #[cfg(feature = "gates")]
            gated_paths: crate::rendering::GatedPaths::of(ops),
        }
    }

    /// A form's description changes what a client view says of it.
    #[cfg_attr(
        not(feature = "clients"),
        expect(clippy::unused_self, reason = "only a client view describes forms")
    )]
    fn forget_forms(&mut self) {
        #[cfg(feature = "clients")]
        self.client_parts.take();
    }
}

impl Template {
    pub(crate) fn new(name: &str, ops: Vec<Op>) -> Self {
        Self {
            precomputed: Precomputed::of(&ops),
            name: name.to_owned(),
            ops,
            document_capacity: AtomicUsize::new(0),
            fragment_capacity: AtomicUsize::new(0),
        }
    }

    #[cfg(feature = "parse")]
    pub fn parse(name: &str, source: &str) -> Result<Self, TemplateError> {
        Self::parse_with(name, source, &crate::Components::default())
    }

    #[cfg(feature = "parse")]
    pub fn parse_with(
        name: &str,
        source: &str,
        components: &crate::Components,
    ) -> Result<Self, TemplateError> {
        crate::compiled::parse_template(name, source, components)
    }

    /// Fails when the template has no such form or a `Field` is not one of the input's fields.
    pub fn describe_form(
        &mut self,
        form_id: &str,
        fields: &'static [FormField],
    ) -> Result<(), TemplateError> {
        self.precomputed.forget_forms();
        match catalog::describe_forms(&mut self.ops, form_id, fields) {
            Ok(true) => Ok(()),
            Ok(false) => Err(TemplateError::unlocated(
                &self.name,
                format!("there is no `Form` with id `{form_id}` to describe"),
            )),
            Err(error) => Err(self.defect(error)),
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn render(&self, data: &(impl Show + ?Sized)) -> Result<String, TemplateError> {
        render_whole(self, data.show(), None, Language::plain())
    }

    pub fn render_submitted(
        &self,
        data: &(impl Show + ?Sized),
        submitted: &Submitted<'_>,
    ) -> Result<String, TemplateError> {
        render_whole(self, data.show(), Some(submitted), Language::plain())
    }

    pub fn render_fragment(
        &self,
        data: &(impl Show + ?Sized),
        id: &str,
    ) -> Result<String, RenderError> {
        render_part(self, data.show(), id, Language::plain())
    }

    /// Marked `hx-swap-oob`, so htmx places it whatever the request targeted.
    pub fn render_out_of_band(
        &self,
        data: &(impl Show + ?Sized),
        id: &str,
    ) -> Result<String, RenderError> {
        self.render_fragment(data, id).map(mark_out_of_band)
    }

    fn defect(&self, error: LocatedError) -> TemplateError {
        TemplateError::located(&self.name, error)
    }
}

/// The fields `Form` `form_id` shows an error under, or `None` when the template has no such
/// form: an error for any other name, or for a post without a form, belongs in the notice.
#[must_use]
pub fn form_fields<'t>(template: &'t Template, form_id: &str) -> Option<Vec<&'t str>> {
    let mut names = None;
    catalog::form_fields(&template.ops, form_id, &mut names);
    names
}

pub(crate) fn render_whole<'a>(
    template: &'a Template,
    data: Shown<'a>,
    submitted: Option<&'a Submitted<'a>>,
    language: Language<'a>,
) -> Result<String, TemplateError> {
    let capacity = template.document_capacity.load(Ordering::Relaxed);
    let html = render::render_document(&template.ops, data, capacity, submitted, language)
        .map_err(|error| template.defect(error))?;
    template
        .document_capacity
        .store(html.len().min(MAXIMUM_CAPACITY_HINT), Ordering::Relaxed);
    Ok(html)
}

pub(crate) fn render_part<'a>(
    template: &'a Template,
    data: Shown<'a>,
    id: &'a str,
    language: Language<'a>,
) -> Result<String, RenderError> {
    // Room for the cleared notice an htmx reply appends, so appending does not copy.
    let capacity = template
        .fragment_capacity
        .load(Ordering::Relaxed)
        .saturating_add(CLEARED_NOTICE.len());
    let html = render::render_fragment(&template.ops, data, id, capacity, language)
        .map_err(|error| RenderError::Template(template.defect(error)))?
        .ok_or_else(|| RenderError::FragmentNotFound { id: id.to_owned() })?;
    template
        .fragment_capacity
        .store(html.len().min(MAXIMUM_CAPACITY_HINT), Ordering::Relaxed);
    Ok(html)
}

pub(crate) fn mark_out_of_band(mut html: String) -> String {
    if let Some(name_length) = html
        .strip_prefix('<')
        .and_then(|rest| rest.find([' ', '>']))
    {
        html.insert_str(1 + name_length, " hx-swap-oob=\"true\"");
    }
    html
}

#[cfg(all(test, feature = "parse"))]
mod tests {
    use std::sync::atomic::Ordering;

    use serde_json::json;

    use super::{MAXIMUM_CAPACITY_HINT, Template};

    #[test]
    fn one_huge_render_does_not_make_every_later_one_reserve_as_much() {
        let template = Template::parse("rows", "<p>{data.text}</p>").unwrap();

        template
            .render(&json!({"text": "x".repeat(4 * MAXIMUM_CAPACITY_HINT)}))
            .unwrap();

        assert_eq!(
            template.document_capacity.load(Ordering::Relaxed),
            MAXIMUM_CAPACITY_HINT
        );
    }
}
