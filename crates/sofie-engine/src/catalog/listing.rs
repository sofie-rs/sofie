//! Without JavaScript the sort links, filter form and pager load the whole page.

use std::borrow::Cow;

use sofie_core::encoding::percent;
use sofie_core::listing::{
    CONTROLS, DEFAULT_SIZE, ERRORS, FILTER, HAS_NEXT, ITEMS, PAGE, QUERY, REFUSAL_ARGUMENTS,
    REFUSAL_KEY, REFUSAL_MESSAGE, SIZE, SORT,
};
use sofie_core::show::Fields;
use sofie_core::{Instant, Shown};

use super::props::{Props, is_element_id, is_field_name};
use super::table::{Table, render_body};
use crate::escape::push_escaped;
use crate::language::{Language, label};
use crate::render::{
    Renderer, Resolved, each_field, field_of, fields_of, items_of, push_display, str_of, text_of,
};
use crate::syntax::{Expression, LocatedError, Position};

#[derive(Debug, Default)]
pub(crate) struct Listing {
    pub(crate) sortable: Vec<String>,
    pub(crate) filters: Vec<(String, Expression)>,
    labels: Vec<Expression>,
}

pub(super) fn build(
    props: &mut Props,
    columns: &[String],
    position: Position,
) -> Result<Listing, LocatedError> {
    let sortable = names(props, "sort")?;
    if let Some(unknown) = sortable.iter().find(|name| !columns.contains(name)) {
        bail!(
            position,
            "`Table` sorts by `{unknown}`, which is not one of its columns"
        );
    }
    let filters = names(props, "filter")?
        .into_iter()
        .map(|name| {
            let label = label(&name, position);
            (name, label)
        })
        .collect();
    let labels = columns.iter().map(|name| label(name, position)).collect();
    Ok(Listing {
        sortable,
        filters,
        labels,
    })
}

fn names(props: &mut Props, prop: &str) -> Result<Vec<String>, LocatedError> {
    let Some(attribute) = props.take(prop) else {
        return Ok(Vec::new());
    };
    let names: Vec<String> = match &attribute.value {
        Expression::Authored(text) => text.split_whitespace().map(str::to_owned).collect(),
        _ => Vec::new(),
    };
    if names.is_empty() || names.iter().any(|name| !is_field_name(name)) {
        bail!(
            attribute.position,
            "`Table` needs `{prop}` as quoted field names, such as `{prop}=\"status total\"`"
        );
    }
    Ok(names)
}

pub(super) struct Listed<'a> {
    items: Resolved<'a>,
    page: Option<u64>,
    size: Option<u64>,
    has_next: bool,
    sort: Option<&'a str>,
    filter: Option<&'a dyn Fields>,
    /// The loader's own query parameters, kept on every link and by the filter form.
    query: Option<&'a dyn Fields>,
    /// Per field, the control its filter takes: `values` to choose from, or an input `type`.
    controls: Option<&'a dyn Fields>,
    errors: Option<&'a dyn Fields>,
}

pub(super) fn listed<'a>(table: &'a Table, renderer: &Renderer<'a>) -> Option<Listed<'a>> {
    Listed::of(table, renderer.resolve(&table.rows))
}

impl<'a> Listed<'a> {
    fn of(table: &Table, of: Resolved<'a>) -> Option<Self> {
        let page = of.filter(|_| field_of(of, ITEMS).is_some());
        if page.is_none() && table.listing.sortable.is_empty() && table.listing.filters.is_empty() {
            return None;
        }
        let field = |name: &str| field_of(page, name);
        Some(Self {
            items: field(ITEMS).or(of),
            page: whole_number(field(PAGE)),
            size: whole_number(field(SIZE)).filter(|&size| size != DEFAULT_SIZE),
            has_next: matches!(field(HAS_NEXT), Some(Shown::Bool(true))),
            sort: str_of(field(SORT)),
            filter: fields_of(field(FILTER)),
            query: fields_of(field(QUERY)),
            controls: fields_of(field(CONTROLS)),
            errors: fields_of(field(ERRORS)),
        })
    }

    /// The first page is left out, so a new order or filter starts there.
    fn push_query(&self, out: &mut String, page: u64, sort: Option<&str>) {
        out.push('?');
        let mut separator = "";
        let mut pair = |out: &mut String, name: &str, value: &str| {
            out.push_str(separator);
            percent::push_encoded(out, name, percent::is_unreserved);
            out.push('=');
            percent::push_encoded(out, value, percent::is_unreserved);
            separator = "&amp;";
        };
        if page > 1 {
            pair(out, PAGE, &page.to_string());
        }
        if let Some(size) = self.size {
            pair(out, SIZE, &size.to_string());
        }
        if let Some(sort) = sort {
            pair(out, SORT, sort);
        }
        let asked = self.query.into_iter().chain(self.filter);
        for (name, value) in asked.flat_map(each_field) {
            if let Some(value) = text_of(Some(value)) {
                pair(out, name, &value);
            }
        }
    }
}

fn whole_number(shown: Resolved<'_>) -> Option<u64> {
    match shown {
        Some(Shown::Number(number)) => number.as_u64(),
        Some(Shown::Integer(number)) => u64::try_from(number).ok(),
        _ => None,
    }
}

pub(super) fn render_listing<'a>(
    table: &'a Table,
    id: Option<&'a Expression>,
    listed: &Listed<'a>,
    renderer: &mut Renderer<'a>,
) -> Result<(), LocatedError> {
    let target = id_text(id, renderer);
    renderer.push_markup("<div class=\"sofie-listing\"");
    renderer.write_id(id);
    renderer.push_markup(">");
    render_unplaced_errors(table, listed, renderer);
    render_filters(table, listed, &target, renderer);
    renderer.push_markup("<table>");
    render_header(table, listed, &target, renderer);
    render_body(table, listed.items, renderer)?;
    renderer.push_markup("</table>");
    render_pager(listed, &target, renderer);
    renderer.push_markup("</div>");
    Ok(())
}

/// A bound id that is no plain id targets nothing rather than a wider selector.
fn id_text<'a>(id: Option<&'a Expression>, renderer: &mut Renderer<'a>) -> String {
    let text = id.map_or("", |id| renderer.plain_text(id));
    if is_element_id(text) {
        text.to_owned()
    } else {
        String::new()
    }
}

/// htmx keeps the URL, so reloading shows the same page.
fn open_link(out: &mut String, listed: &Listed<'_>, target: &str, page: u64, sort: Option<&str>) {
    let mut query = String::new();
    listed.push_query(&mut query, page, sort);
    out.push_str("<a href=\"");
    out.push_str(&query);
    out.push_str("\" hx-get=\"");
    out.push_str(&query);
    push_fetch_attributes(out, target);
    out.push('>');
}

fn push_fetch_attributes(out: &mut String, target: &str) {
    out.push_str("\" hx-target=\"#");
    out.push_str(target);
    out.push_str("\" hx-swap=\"outerHTML\" hx-push-url=\"true\"");
}

fn render_header<'a>(
    table: &'a Table,
    listed: &Listed<'a>,
    target: &str,
    renderer: &mut Renderer<'a>,
) {
    renderer.push_markup("<thead><tr>");
    for (column, label) in table.columns.iter().zip(&table.listing.labels) {
        let sorted = match listed.sort {
            Some(sort) if sort == column => Some("ascending"),
            Some(sort) if sort.strip_prefix('-') == Some(column) => Some("descending"),
            _ => None,
        };
        let out = renderer.output();
        out.push_str("<th scope=\"col\"");
        if let Some(sorted) = sorted {
            out.push_str(" aria-sort=\"");
            out.push_str(sorted);
            out.push('"');
        }
        out.push('>');
        let sortable = table.listing.sortable.contains(column);
        if sortable {
            let next = match sorted {
                Some("ascending") => format!("-{column}"),
                _ => column.clone(),
            };
            open_link(out, listed, target, 1, Some(&next));
        }
        renderer.write_text(label);
        if sortable {
            renderer.push_markup("</a>");
        }
        renderer.push_markup("</th>");
    }
    if !table.row_content.is_empty() {
        renderer.push_markup("<th scope=\"col\"></th>");
    }
    renderer.push_markup("</tr></thead>");
}

/// An operator filter, or one a Table offers no field for, has no slot to show its error beside.
fn render_unplaced_errors(table: &Table, listed: &Listed<'_>, renderer: &mut Renderer<'_>) {
    let language = renderer.language;
    let unplaced = listed
        .errors
        .into_iter()
        .flat_map(each_field)
        .filter(|(key, _)| !table.listing.filters.iter().any(|(name, _)| name == key))
        .filter_map(|(key, error)| Some((key, refusal_text(language, Some(error))?)));
    let out = renderer.output();
    for (key, error) in unplaced {
        out.push_str("<p class=\"sofie-field-error\" role=\"alert\">");
        push_escaped(out, key);
        out.push_str(": ");
        push_escaped(out, &error);
        out.push_str("</p>");
    }
}

/// A refusal shown with its key reads in the viewer's language; plain text is written as it is.
fn refusal_text(language: Language<'_>, refusal: Resolved<'_>) -> Option<String> {
    let Some(fields) = fields_of(refusal) else {
        return text_of(refusal).map(Cow::into_owned);
    };
    let message = text_of(fields.get(REFUSAL_MESSAGE))?;
    let Some(key) = str_of(fields.get(REFUSAL_KEY)) else {
        return Some(message.into_owned());
    };
    let arguments: Vec<(&str, String)> = fields_of(fields.get(REFUSAL_ARGUMENTS))
        .into_iter()
        .flat_map(each_field)
        .filter_map(|(name, value)| Some((name, text_of(Some(value))?.into_owned())))
        .collect();
    Some(language.field_error(key, &arguments, &message))
}

fn render_filters<'a>(
    table: &'a Table,
    listed: &Listed<'a>,
    target: &str,
    renderer: &mut Renderer<'a>,
) {
    if table.listing.filters.is_empty() {
        return;
    }
    let out = renderer.output();
    out.push_str("<form class=\"sofie-filter\" method=\"get\" hx-get=\"?");
    push_fetch_attributes(out, target);
    out.push('>');
    for (name, label) in &table.listing.filters {
        renderer.push_markup("<label>");
        renderer.write_text(label);
        let value = text_of(listed.filter.and_then(|filter| filter.get(name)));
        let control = fields_of(listed.controls.and_then(|controls| controls.get(name)));
        let error = refusal_text(
            renderer.language,
            listed.errors.and_then(|errors| errors.get(name)),
        );
        let out = renderer.output();
        out.push(' ');
        push_filter_control(out, name, value.as_deref(), control, error.is_some());
        if let Some(error) = error {
            out.push_str("<span class=\"sofie-field-error\">");
            push_escaped(out, &error);
            out.push_str("</span>");
        }
        out.push_str("</label>");
    }
    let out = renderer.output();
    if let Some(size) = listed.size {
        push_hidden(out, SIZE, &size.to_string());
    }
    if let Some(sort) = listed.sort {
        push_hidden(out, SORT, sort);
    }
    for (name, value) in listed.query.into_iter().flat_map(each_field) {
        if let Some(value) = text_of(Some(value)) {
            push_hidden(out, name, &value);
        }
    }
    let text = renderer.language.text("table.filter", "Filter");
    let out = renderer.output();
    out.push_str("<button type=\"submit\">");
    push_escaped(out, text);
    out.push_str("</button></form>");
}

fn push_hidden(out: &mut String, name: &str, value: &str) {
    out.push_str("<input type=\"hidden\" name=\"");
    push_escaped(out, name);
    out.push_str("\" value=\"");
    push_escaped(out, value);
    out.push_str("\">");
}

/// An enum's variants are chosen from; any `type` but `datetime-local` is a search box; an
/// instant given with its offset, or refused, is a text box, which holds the offset a picker
/// would drop.
fn push_filter_control(
    out: &mut String,
    name: &str,
    value: Option<&str>,
    control: Option<&dyn Fields>,
    is_refused: bool,
) {
    let values: Vec<_> = items_of(control.and_then(|control| control.get("values")))
        .filter_map(|choice| text_of(Some(choice)))
        .collect();
    if !values.is_empty() {
        out.push_str("<select name=\"");
        out.push_str(name);
        out.push('"');
        push_invalid(out, is_refused);
        out.push_str("><option value=\"\"></option>");
        for choice in &values {
            super::field::write_option(out, choice, choice, value);
        }
        out.push_str("</select>");
        return;
    }
    let is_date_time =
        str_of(control.and_then(|control| control.get("type"))) == Some("datetime-local");
    let is_picker = value.is_none_or(Instant::is_wall_clock) && !is_refused;
    out.push_str(match (is_date_time, is_picker) {
        (false, _) => "<input type=\"search\" name=\"",
        (true, false) => "<input type=\"text\" name=\"",
        (true, true) => "<input type=\"datetime-local\" name=\"",
    });
    out.push_str(name);
    out.push('"');
    if let Some(value) = value {
        out.push_str(" value=\"");
        push_escaped(out, value);
        out.push('"');
    }
    push_invalid(out, is_refused);
    out.push('>');
}

fn push_invalid(out: &mut String, is_refused: bool) {
    if is_refused {
        out.push_str(" aria-invalid=\"true\"");
    }
}

fn render_pager(listed: &Listed<'_>, target: &str, renderer: &mut Renderer<'_>) {
    let Some(page) = listed.page else {
        return;
    };
    if page <= 1 && !listed.has_next {
        return;
    }
    let previous = renderer.language.text("table.previous", "Previous");
    let next = renderer.language.text("table.next", "Next");
    let out = renderer.output();
    out.push_str("<nav class=\"sofie-pager\">");
    if page > 1 {
        open_link(out, listed, target, page - 1, listed.sort);
        push_escaped(out, previous);
        out.push_str("</a>");
    }
    out.push_str("<span>");
    push_display(out, page);
    out.push_str("</span>");
    if listed.has_next
        && let Some(next_page) = page.checked_add(1)
    {
        open_link(out, listed, target, next_page, listed.sort);
        push_escaped(out, next);
        out.push_str("</a>");
    }
    out.push_str("</nav>");
}
