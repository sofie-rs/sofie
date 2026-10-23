use std::borrow::Cow;

use sofie_core::{DisplayZone, Instant, Show, Shown};
use sofie_values::{Constraint, Control, Currency, FormField, Input, Part};

use super::form::Submitted;
use super::money::money_of;
use super::props::{Props, check_path, check_single, is_element_id, is_field_name};
use crate::escape::push_escaped;
use crate::label::humanize;
use crate::language::label;
use crate::render::{
    Renderer, Resolved, field_of, items_of, push_display, str_of, text_of, write_escaped,
    write_plain,
};
use crate::syntax::{Attribute, Expression, LocatedError, Position};

const TEXT: Input = Input::typed(&[("type", "text")]);

#[derive(Debug)]
pub(crate) struct Field {
    pub name: String,
    pub label: Expression,
    described: Option<&'static FormField>,
    pub value: Option<Attribute>,
    pub options: Option<Attribute>,
    pub currency: Option<Attribute>,
    is_required: bool,
    pub position: Position,
}

impl Field {
    pub(crate) fn is_file(&self) -> bool {
        self.input().control == Control::File
    }

    fn input(&self) -> Input {
        self.described.map_or(TEXT, |described| described.input)
    }

    fn is_required(&self) -> bool {
        self.described.map_or(self.is_required, |described| {
            described.required || described.constraints.contains(&Constraint::Required)
        })
    }

    /// Only the template's own `required`: the input type's is given at runtime.
    #[cfg(feature = "client-compile")]
    pub(crate) fn is_written_required(&self) -> bool {
        self.is_required
    }

    /// What a client area renders the field from but its label, which is in the request's.
    #[cfg(feature = "clients")]
    pub(crate) fn client_description(&self) -> serde_json::Value {
        let input = self.input();
        let pairs = |attributes: &[(&str, &str)]| -> Vec<[String; 2]> {
            attributes
                .iter()
                .map(|(name, value)| [(*name).to_owned(), (*value).to_owned()])
                .collect()
        };
        let parts: Vec<serde_json::Value> = match input.control {
            Control::Parts(parts) => parts
                .iter()
                .map(|part| {
                    serde_json::json!({
                        "name": part.name,
                        "label": humanize(part.name),
                        "attributes": pairs(part.attributes),
                        "options": part.options,
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
        let constraints: Vec<(&str, i64)> = self
            .constraints()
            .iter()
            .filter_map(|constraint| constraint_attribute(*constraint))
            .collect();
        serde_json::json!({
            "name": self.name,
            "control": control_name(input.control),
            "attributes": pairs(input.attributes),
            "options": input.options,
            "parts": parts,
            "required": self.is_required(),
            "constraints": constraints,
        })
    }

    fn constraints(&self) -> &'static [Constraint] {
        self.described.map_or(&[], |field| field.constraints)
    }

    fn value(&self) -> Option<&Expression> {
        self.value.as_ref().map(|value| &value.value)
    }

    pub(super) fn describe(&mut self, field: &'static FormField) -> Result<(), LocatedError> {
        check_props(self, field.input)?;
        self.described = Some(field);
        Ok(())
    }

    pub(super) fn of(field: &'static FormField, position: Position) -> Self {
        Self {
            name: field.name.to_owned(),
            label: label(field.name, position),
            described: Some(field),
            value: None,
            options: None,
            currency: None,
            is_required: false,
            position,
        }
    }
}

#[cfg(feature = "clients")]
fn control_name(control: Control) -> &'static str {
    match control {
        Control::Input => "input",
        Control::Password => "password",
        Control::Textarea => "textarea",
        Control::Checkbox => "checkbox",
        Control::Select => "select",
        Control::Money => "money",
        Control::Percentage => "percentage",
        Control::Parts(_) => "parts",
        Control::File => "file",
    }
}

/// A label bound to data is the template's to write; the client compiles it.
#[cfg(feature = "clients")]
pub(crate) fn constant_label(
    label: &Expression,
    language: crate::language::Language<'_>,
) -> Option<String> {
    match label {
        Expression::Authored(text) => Some(text.clone()),
        #[cfg(feature = "i18n")]
        Expression::Call(call) => match call.as_ref() {
            crate::translations::Call::Label { key, humanized, .. } => {
                Some(language.text(key, humanized).to_owned())
            }
            _ => None,
        },
        _ => {
            let _ = language;
            None
        }
    }
}

pub(super) fn build_field(props: &mut Props, position: Position) -> Result<Field, LocatedError> {
    let (name, name_position) = props.required_literal("name")?;
    if !is_field_name(&name) {
        bail!(
            name_position,
            "`Field` name `{name}` is not a field name (letters, digits and `_`)"
        );
    }
    Ok(Field {
        label: props
            .optional("label")
            .unwrap_or_else(|| label(&name, position)),
        described: None,
        value: props.take("value"),
        options: props.take("options"),
        currency: props.take("currency"),
        is_required: props.flag("required")?,
        position,
        name,
    })
}

fn check_props(field: &Field, input: Input) -> Result<(), LocatedError> {
    let reads_value = matches!(
        input.control,
        Control::Money | Control::Select | Control::Checkbox | Control::Parts(_)
    );
    for attribute in [&field.value, &field.options, &field.currency]
        .into_iter()
        .flatten()
    {
        if reads_value || attribute.name != "value" {
            check_single("Field", attribute)?;
        }
    }
    let unused = match input.control {
        Control::Password if field.value.is_some() => {
            bail!(
                field.position,
                "a `password` field takes no `value`: a password is never sent back to the page"
            );
        }
        Control::File if field.value.is_some() => {
            bail!(field.position, "a file field takes no `value`");
        }
        Control::Select if field.options.is_none() && input.options.is_empty() => {
            bail!(field.position, "`Field` needs the `options` prop");
        }
        Control::Money => {
            if let Some(value) = &field.value {
                check_path("Field", value)?;
            }
            check_currency(field)?;
            &field.options
        }
        Control::Parts(_) => {
            if let Some(value) = &field.value {
                check_path("Field", value)?;
            }
            if field.options.is_some() {
                &field.options
            } else {
                &field.currency
            }
        }
        Control::Select => &field.currency,
        _ if field.currency.is_some() => &field.currency,
        _ => &field.options,
    };
    match unused {
        Some(attribute) => Err(LocatedError::new(
            attribute.position,
            format!("`Field` has no prop `{}`", attribute.name),
        )),
        None => Ok(()),
    }
}

/// A bound currency, or a money `value`'s, is checked when rendering; a null one is typed.
fn check_currency(field: &Field) -> Result<(), LocatedError> {
    match &field.currency {
        Some(Attribute {
            value: Expression::Authored(code),
            position,
            ..
        }) if Currency::parse(code).is_err() => Err(LocatedError::new(
            *position,
            format!("currency `{code}` is not an ISO 4217 code such as EUR"),
        )),
        _ => Ok(()),
    }
}

#[derive(Clone, Copy)]
enum Source<'a> {
    Posted(Resolved<'a>),
    Bound(Option<&'a Expression>),
}

impl<'a> Source<'a> {
    fn is_present(self) -> bool {
        matches!(self, Self::Posted(Some(_)) | Self::Bound(Some(_)))
    }

    fn write(self, renderer: &mut Renderer<'a>) {
        if let Self::Bound(Some(value)) = self {
            return renderer.write_text(value);
        }
        let resolved = self.resolve(renderer);
        write_escaped(renderer.output(), resolved);
    }

    fn resolve(self, renderer: &Renderer<'a>) -> Resolved<'a> {
        match self {
            Self::Posted(value) => value,
            Self::Bound(value) => value.and_then(|value| renderer.resolve(value)),
        }
    }
}

pub(super) fn render_field<'a>(
    field: &'a Field,
    form_id: &str,
    submitted: Option<&Submitted<'a>>,
    renderer: &mut Renderer<'a>,
) -> Result<(), LocatedError> {
    let error = submitted.and_then(|submitted| submitted.error(&field.name));
    let input = field.input();
    let source = match submitted {
        Some(_) if matches!(input.control, Control::Password | Control::File) => {
            Source::Posted(None)
        }
        Some(submitted) => Source::Posted(submitted.input.get(&field.name).map(Show::show)),
        None => Source::Bound(field.value()),
    };
    renderer.push_markup("<div class=\"sofie-field\">");
    if !input.attributes.contains(&("type", "hidden")) {
        let out = renderer.output();
        out.push_str("<label for=\"");
        push_field_id(out, form_id, &field.name);
        out.push_str("\">");
        renderer.write_text(&field.label);
        renderer.push_markup("</label>");
    }
    let attributes = ControlAttributes {
        field,
        form_id,
        is_invalid: error.is_some(),
    };
    match input.control {
        Control::Input => render_input(&attributes, input.attributes, source, renderer),
        Control::Password => render_input(&attributes, PASSWORD, source, renderer),
        Control::Textarea => render_textarea(&attributes, source, renderer),
        Control::Checkbox => render_checkbox(&attributes, source, renderer),
        Control::Select => render_select(&attributes, input.options, source, renderer),
        Control::Money => render_money_input(&attributes, source, renderer)?,
        Control::Percentage => render_percentage_input(&attributes, source, renderer),
        Control::Parts(parts) => render_parts(&attributes, parts, source, renderer),
        Control::File => render_input(&attributes, FILE, source, renderer),
    }
    let out = renderer.output();
    out.push_str("<p class=\"sofie-field-error\" id=\"");
    push_slot_id(out, form_id, &field.name);
    out.push_str("\" aria-live=\"polite\">");
    push_escaped(out, error.unwrap_or_default());
    out.push_str("</p></div>");
    Ok(())
}

const PASSWORD: &[(&str, &str)] = &[("type", "password")];
const FILE: &[(&str, &str)] = &[("type", "file")];

struct ControlAttributes<'c, 'a> {
    field: &'a Field,
    form_id: &'c str,
    is_invalid: bool,
}

impl ControlAttributes<'_, '_> {
    fn push(&self, out: &mut String, name_suffix: &str) {
        self.push_id(out);
        self.push_name(out, name_suffix);
        if self.field.is_required() {
            out.push_str(" required");
        }
        for (name, bound) in self
            .field
            .constraints()
            .iter()
            .filter_map(|constraint| constraint_attribute(*constraint))
        {
            out.push(' ');
            out.push_str(name);
            out.push_str("=\"");
            push_display(out, bound);
            out.push('"');
        }
        if self.is_invalid {
            out.push_str(" aria-invalid=\"true\"");
        }
    }

    /// Only the first part carries the id the label points to; constraints describe the whole
    /// value, so no part takes them.
    fn push_part(&self, out: &mut String, part: &str, is_first: bool) {
        if is_first {
            self.push_id(out);
        } else {
            out.push_str(" aria-label=\"");
            push_escaped(out, &humanize(part));
            out.push('"');
        }
        self.push_name(out, &format!(".{part}"));
        if self.field.is_required() {
            out.push_str(" required");
        }
        if self.is_invalid {
            out.push_str(" aria-invalid=\"true\"");
        }
    }

    fn push_id(&self, out: &mut String) {
        out.push_str(" id=\"");
        push_field_id(out, self.form_id, &self.field.name);
        out.push('"');
    }

    fn push_name(&self, out: &mut String, name_suffix: &str) {
        let name = &self.field.name;
        out.push_str(" name=\"");
        out.push_str(name);
        out.push_str(name_suffix);
        out.push_str("\" aria-describedby=\"");
        push_slot_id(out, self.form_id, name);
        out.push('"');
    }
}

/// Only the database can check `Unique`; `Required` comes from the field's own `required`.
fn constraint_attribute(constraint: Constraint) -> Option<(&'static str, i64)> {
    match constraint {
        Constraint::MinLength(bound) => Some(("minlength", bound)),
        Constraint::MaxLength(bound) => Some(("maxlength", bound)),
        Constraint::Min(bound) => Some(("min", bound)),
        Constraint::Max(bound) => Some(("max", bound)),
        Constraint::Required | Constraint::Unique => None,
    }
}

/// The input's own attributes come from code, yet are escaped like data.
fn render_input<'a>(
    attributes: &ControlAttributes<'_, '_>,
    input_attributes: &[(&str, &str)],
    source: Source<'a>,
    renderer: &mut Renderer<'a>,
) {
    let zone = DisplayZone::current();
    let instant = text_of(source.resolve(renderer))
        .filter(|_| input_attributes.contains(&("type", "datetime-local")))
        .and_then(|value| match source {
            Source::Bound(_) => instant_control(&value, zone),
            Source::Posted(_) => Some(posted_instant_control(value.into_owned(), zone)),
        });
    let out = renderer.output();
    out.push_str("<input");
    match instant {
        Some(InstantControl::WithOffset(_)) => {
            let others = input_attributes.iter().filter(|(name, _)| *name != "type");
            push_input_attributes(out, [("type", "text")].iter().chain(others));
        }
        Some(InstantControl::Second(_)) => {
            push_input_attributes(out, input_attributes);
            out.push_str(" step=\"any\"");
        }
        Some(InstantControl::Minute(_)) | None => push_input_attributes(out, input_attributes),
    }
    attributes.push(out, "");
    if !source.is_present() {
        renderer.push_markup(">");
        return;
    }
    renderer.push_markup(" value=\"");
    match instant {
        Some(
            InstantControl::Minute(value)
            | InstantControl::Second(value)
            | InstantControl::WithOffset(value),
        ) => {
            push_escaped(renderer.output(), &value);
        }
        None => source.write(renderer),
    }
    renderer.push_markup("\">");
}

fn push_input_attributes<'s>(
    out: &mut String,
    input_attributes: impl IntoIterator<Item = &'s (&'s str, &'s str)>,
) {
    for (name, value) in input_attributes {
        out.push(' ');
        out.push_str(name);
        out.push_str("=\"");
        push_escaped(out, value);
        out.push('"');
    }
}

/// An instant in a `datetime-local` control, on the wall clock `Instant::parse` reads it back on.
#[derive(Debug, PartialEq)]
enum InstantControl {
    Minute(String),
    /// Needs `step="any"`, or the browser refuses the seconds.
    Second(String),
    /// A text box: a wall clock the zone repeats, which a `datetime-local` control can't tell
    /// apart, or a fraction finer than the milliseconds it keeps.
    WithOffset(String),
}

fn instant_control(value: &str, zone: &DisplayZone) -> Option<InstantControl> {
    let is_offset_time = value.ends_with('Z') && value.as_bytes().get(10) == Some(&b'T');
    if !is_offset_time {
        return None;
    }
    let instant = Instant::parse(value).ok()?;
    let wall_clock = zone.wall_clock(instant);
    if zone.repeats(wall_clock) || wall_clock.subsec_nanosecond() % 1_000_000 != 0 {
        return Some(InstantControl::WithOffset(zone.with_offset(instant)));
    }
    if wall_clock.second() == 0 && wall_clock.subsec_nanosecond() == 0 {
        return Some(InstantControl::Minute(format!(
            "{}T{:02}:{:02}",
            wall_clock.date(),
            wall_clock.hour(),
            wall_clock.minute()
        )));
    }
    Some(InstantControl::Second(wall_clock.to_string()))
}

/// Shown again as typed, in the control that holds it. A text box holds any text that is no wall
/// clock, a wall clock the zone repeats or that has a fraction finer than milliseconds, and one the
/// server cannot read in the zone; the client, which cannot read it, skips that last test.
fn posted_instant_control(text: String, zone: &DisplayZone) -> InstantControl {
    if !Instant::is_wall_clock(&text) {
        return InstantControl::WithOffset(text);
    }
    let fraction_digits = text
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len());
    if fraction_digits > 3 || Instant::parse_in(&text, zone).is_err() {
        return InstantControl::WithOffset(text);
    }
    if text.len() == 16 {
        InstantControl::Minute(text)
    } else {
        InstantControl::Second(text)
    }
}

fn render_textarea<'a>(
    attributes: &ControlAttributes<'_, '_>,
    source: Source<'a>,
    renderer: &mut Renderer<'a>,
) {
    let out = renderer.output();
    out.push_str("<textarea");
    attributes.push(out, "");
    // The parser drops one newline after the start tag, so a value's own leading newline stays.
    out.push_str(">\n");
    source.write(renderer);
    renderer.push_markup("</textarea>");
}

/// The hidden `false` comes first, so an unchecked box still posts and a checked one overrides it.
/// Only `true` checks a bound box: the text `"false"` would be truthy.
fn render_checkbox<'a>(
    attributes: &ControlAttributes<'_, '_>,
    source: Source<'a>,
    renderer: &mut Renderer<'a>,
) {
    let is_checked = match source {
        Source::Posted(value) => str_of(value) == Some("true"),
        Source::Bound(_) => matches!(source.resolve(renderer), Some(Shown::Bool(true))),
    };
    let out = renderer.output();
    out.push_str("<input type=\"hidden\" name=\"");
    out.push_str(&attributes.field.name);
    out.push_str("\" value=\"false\"><input type=\"checkbox\"");
    attributes.push(out, "");
    out.push_str(" value=\"true\"");
    if is_checked {
        out.push_str(" checked");
    }
    out.push('>');
}

/// An optional select starts with an empty choice, so nothing need be chosen.
fn render_select<'a>(
    attributes: &ControlAttributes<'_, 'a>,
    own_options: &[&str],
    source: Source<'a>,
    renderer: &mut Renderer<'a>,
) {
    let options = attributes
        .field
        .options
        .as_ref()
        .map(|options| &options.value);
    let selected = choice_of(source.resolve(renderer));
    let selected = selected.as_deref();
    let choices: Vec<Cow<'_, str>> = match options {
        Some(options) => items_of(renderer.resolve(options))
            .filter_map(|item| choice_of(Some(item)))
            .collect(),
        None => own_options.iter().copied().map(Cow::Borrowed).collect(),
    };
    let out = renderer.output();
    out.push_str("<select");
    attributes.push(out, "");
    out.push('>');
    if !attributes.field.is_required() {
        out.push_str("<option value=\"\"></option>");
    }
    #[cfg(feature = "i18n")]
    super::badge::write_enum_options(renderer, options, selected);
    for choice in &choices {
        write_option(renderer.output(), choice, choice, selected);
    }
    renderer.push_markup("</select>");
}

/// Numbers are offered too, written as a form posts them back.
fn choice_of(resolved: Resolved<'_>) -> Option<Cow<'_, str>> {
    match resolved {
        Some(Shown::Integer(_) | Shown::Float(_) | Shown::Number(_)) => {
            let mut text = String::new();
            write_plain(&mut text, resolved);
            Some(Cow::Owned(text))
        }
        _ => text_of(resolved),
    }
}

pub(super) fn write_option(out: &mut String, value: &str, label: &str, selected: Option<&str>) {
    out.push_str("<option value=\"");
    push_escaped(out, value);
    out.push('"');
    if selected == Some(value) {
        out.push_str(" selected");
    }
    out.push('>');
    push_escaped(out, label);
    out.push_str("</option>");
}

/// A currency is three letters and a formatted amount digits, `-` and `.`; only a posted amount
/// needs escaping.
fn render_money_input<'a>(
    attributes: &ControlAttributes<'_, 'a>,
    source: Source<'a>,
    renderer: &mut Renderer<'a>,
) -> Result<(), LocatedError> {
    let field = attributes.field;
    let amount = money_of(Source::Bound(field.value()).resolve(renderer));
    let currency = match amount {
        Some(amount) => Some(amount.currency()),
        None => bound_currency(field, renderer)?,
    };
    let posted = |part: &str| match source {
        Source::Posted(posted) => Some(str_of(field_of(posted, part)).unwrap_or_default()),
        Source::Bound(_) => None,
    };
    let currency_label = renderer.language.text("field.currency", "Currency");
    let out = renderer.output();
    out.push_str("<div class=\"sofie-money-input\"><input type=\"text\" inputmode=\"decimal\"");
    attributes.push(out, ".amount");
    match (posted("amount"), amount) {
        (Some(typed), _) => push_value(out, typed),
        (None, Some(amount)) => {
            out.push_str(" value=\"");
            amount.write_major(out);
            out.push('"');
        }
        (None, None) => {}
    }
    out.push('>');
    push_currency(
        out,
        &field.name,
        currency,
        posted("currency"),
        currency_label,
    );
    out.push_str("</div>");
    Ok(())
}

/// A bound value is basis points; a posted one is shown as it was typed.
fn render_percentage_input<'a>(
    attributes: &ControlAttributes<'_, 'a>,
    source: Source<'a>,
    renderer: &mut Renderer<'a>,
) {
    let resolved = source.resolve(renderer);
    let basis_points = match (source, resolved) {
        (Source::Bound(_), Some(Shown::Integer(basis_points))) => Some(basis_points),
        (Source::Bound(_), Some(Shown::Number(number))) => number.as_i64(),
        _ => None,
    };
    let out = renderer.output();
    out.push_str(
        "<div class=\"sofie-percentage-input\"><input type=\"text\" inputmode=\"decimal\"",
    );
    attributes.push(out, "");
    if let Some(basis_points) = basis_points {
        let sign = if basis_points < 0 { "-" } else { "" };
        let magnitude = basis_points.unsigned_abs();
        out.push_str(" value=\"");
        push_display(
            out,
            format_args!("{sign}{}.{:02}", magnitude / 100, magnitude % 100),
        );
        out.push('"');
    } else {
        push_resolved_value(out, resolved);
    }
    out.push_str("><span class=\"sofie-unit\">%</span></div>");
}

fn render_parts<'a>(
    attributes: &ControlAttributes<'_, 'a>,
    parts: &[Part],
    source: Source<'a>,
    renderer: &mut Renderer<'a>,
) {
    let resolved = source.resolve(renderer);
    let out = renderer.output();
    out.push_str("<div class=\"sofie-parts\">");
    for (index, part) in parts.iter().enumerate() {
        let value = field_of(resolved, part.name);
        if part.options.is_empty() {
            out.push_str("<input");
            push_input_attributes(out, part.attributes);
            attributes.push_part(out, part.name, index == 0);
            push_resolved_value(out, value);
            out.push('>');
            continue;
        }
        out.push_str("<select");
        attributes.push_part(out, part.name, index == 0);
        out.push('>');
        if !attributes.field.is_required() {
            out.push_str("<option value=\"\"></option>");
        }
        let selected = text_of(value);
        for option in part.options {
            write_option(out, option, option, selected.as_deref());
        }
        out.push_str("</select>");
    }
    out.push_str("</div>");
}

fn push_currency(
    out: &mut String,
    name: &str,
    currency: Option<Currency>,
    typed: Option<&str>,
    label: &str,
) {
    let Some(currency) = currency else {
        out.push_str("<input type=\"text\" class=\"sofie-currency\" maxlength=\"3\" aria-label=\"");
        push_escaped(out, label);
        out.push_str("\" name=\"");
        out.push_str(name);
        out.push_str(".currency\"");
        push_value(out, typed.unwrap_or_default());
        out.push('>');
        return;
    };
    out.push_str("<span class=\"sofie-currency\">");
    out.push_str(currency.as_str());
    out.push_str("</span><input type=\"hidden\" name=\"");
    out.push_str(name);
    out.push_str(".currency\" value=\"");
    out.push_str(currency.as_str());
    out.push_str("\">");
}

fn push_value(out: &mut String, value: &str) {
    if value.is_empty() {
        return;
    }
    out.push_str(" value=\"");
    push_escaped(out, value);
    out.push('"');
}

fn push_resolved_value(out: &mut String, resolved: Resolved<'_>) {
    if matches!(resolved, None | Some(Shown::Null)) {
        return;
    }
    out.push_str(" value=\"");
    write_escaped(out, resolved);
    out.push('"');
}

fn bound_currency<'a>(
    field: &'a Field,
    renderer: &Renderer<'a>,
) -> Result<Option<Currency>, LocatedError> {
    let Some(currency) = &field.currency else {
        return Ok(None);
    };
    let resolved = renderer.resolve(&currency.value);
    if matches!(resolved, Some(Shown::Null)) {
        return Ok(None);
    }
    let code = text_of(resolved);
    Currency::parse(code.as_deref().unwrap_or_default()).map(Some).map_err(|_| {
        LocatedError::new(
            field.position,
            format!(
                "money field `{}` has no currency: its `value` is not money and its `currency` is not an ISO 4217 code",
                field.name
            ),
        )
    })
}

/// Escaped though callers pass checked ids, so a missed check cannot open the attribute.
fn push_field_id(out: &mut String, form_id: &str, name: &str) {
    push_escaped(out, form_id);
    out.push('-');
    out.push_str(name);
}

fn push_slot_id(out: &mut String, form_id: &str, name: &str) {
    push_field_id(out, form_id, name);
    out.push_str("-error");
}

/// A form id or field that is not a plain name is skipped, so request data cannot widen selectors.
#[must_use]
pub fn field_errors(form_id: &str, errors: &[(&str, String)]) -> String {
    let mut html = String::new();
    if !is_element_id(form_id) {
        return html;
    }
    html.push_str("<div hx-swap-oob=\"innerHTML:#");
    push_escaped(&mut html, form_id);
    html.push_str(" .sofie-field-error\"></div>");
    for (field, message) in errors.iter().filter(|(field, _)| is_field_name(field)) {
        html.push_str("<div hx-swap-oob=\"innerHTML:#");
        push_slot_id(&mut html, form_id, field);
        html.push_str("\">");
        push_escaped(&mut html, message);
        html.push_str("</div>");
    }
    html
}

#[cfg(test)]
mod tests {
    use sofie_core::DisplayZone;

    use super::{InstantControl, instant_control, push_slot_id};

    #[test]
    fn a_date_time_control_shows_an_instant_on_the_display_zone_s_wall_clock() {
        let zone = DisplayZone::named("Europe/Istanbul").unwrap();

        let minutes = instant_control("2026-10-11T22:30:00Z", &zone);
        let seconds = instant_control("2026-10-11T22:30:15.5Z", &zone);

        assert_eq!(
            minutes,
            Some(InstantControl::Minute("2026-10-12T01:30".to_owned()))
        );
        assert_eq!(
            seconds,
            Some(InstantControl::Second("2026-10-12T01:30:15.5".to_owned()))
        );
    }

    #[test]
    fn an_id_is_escaped_even_if_a_check_is_missed() {
        let mut out = String::new();

        push_slot_id(&mut out, "f\" onclick=\"x", "name");

        assert_eq!(out, "f&quot; onclick=&quot;x-name-error");
    }
}
