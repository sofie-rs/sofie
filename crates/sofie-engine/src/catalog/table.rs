use sofie_core::Shown;

use super::money::money_of;
use super::props::{Props, is_field_name};
use crate::escape::push_escaped;
use crate::language::label;
use crate::program::Program;
use crate::render::{Renderer, Resolved, field_of, items_of, push_display, write_escaped};
use crate::syntax::{Expression, LocatedError, Op, Position};

#[derive(Debug)]
pub(crate) struct Table {
    pub rows: Expression,
    pub columns: Vec<String>,
    /// Where each row's first cell links to, read with the row named by `as`.
    pub link: Option<Expression>,
    pub row_content: Vec<Op>,
    pub header: Vec<Op>,
    #[cfg(feature = "listing")]
    pub listing: super::listing::Listing,
}

pub(super) fn build_table(
    props: &mut Props,
    children: Vec<Op>,
    position: Position,
) -> Result<Table, LocatedError> {
    let rows = props.required_path("of")?;
    let (columns_literal, columns_position) = props.required_literal("columns")?;
    let columns = parse_columns(&columns_literal, columns_position)?;
    let names_row = props.take("as").is_some();
    let link = props.optional("link");
    if link.is_some() && !names_row {
        bail!(
            position,
            "`Table` needs `as=\"…\"` to name the row its `link` reads"
        );
    }
    let row_content = if is_blank(&children) {
        Vec::new()
    } else {
        children
    };
    if !row_content.is_empty() && !names_row {
        bail!(
            position,
            "`Table` needs `as=\"…\"` to name the row its content is rendered for"
        );
    }
    let header = header_ops(&columns, !row_content.is_empty(), columns_position);
    Ok(Table {
        #[cfg(feature = "listing")]
        listing: super::listing::build(props, &columns, columns_position)?,
        rows,
        columns,
        link,
        row_content,
        header,
    })
}

fn is_blank(children: &[Op]) -> bool {
    children
        .iter()
        .all(|child| matches!(child, Op::Markup(text) if text.trim().is_empty()))
}

fn parse_columns(literal: &str, position: Position) -> Result<Vec<String>, LocatedError> {
    let columns: Vec<String> = literal.split_whitespace().map(str::to_owned).collect();
    if columns.is_empty() {
        bail!(position, "`Table` needs at least one column");
    }
    if let Some(invalid) = columns.iter().find(|column| !is_field_name(column)) {
        bail!(
            position,
            "column `{invalid}` is not a field name (letters, digits and `_`)"
        );
    }
    Ok(columns)
}

fn header_ops(columns: &[String], has_row_content: bool, position: Position) -> Vec<Op> {
    let mut header = Program::default();
    header.push_markup("<thead><tr>");
    for column in columns {
        header.push_markup("<th scope=\"col\">");
        header.push_text(label(column, position));
        header.push_markup("</th>");
    }
    if has_row_content {
        header.push_markup("<th scope=\"col\"></th>");
    }
    header.push_markup("</tr></thead>");
    header.into_ops()
}

pub(super) fn render_table<'a>(
    table: &'a Table,
    id: Option<&'a Expression>,
    renderer: &mut Renderer<'a>,
) -> Result<(), LocatedError> {
    #[cfg(feature = "listing")]
    if let Some(listed) = super::listing::listed(table, renderer) {
        return super::listing::render_listing(table, id, &listed, renderer);
    }
    renderer.push_markup("<table");
    renderer.write_id(id);
    renderer.push_markup(">");
    renderer.render_ops(&table.header)?;
    render_body(table, renderer.resolve(&table.rows), renderer)?;
    renderer.push_markup("</table>");
    Ok(())
}

pub(super) fn render_body<'a>(
    table: &'a Table,
    rows: Resolved<'a>,
    renderer: &mut Renderer<'a>,
) -> Result<(), LocatedError> {
    renderer.push_markup("<tbody>");
    let mut rows = items_of(rows).peekable();
    if rows.peek().is_none() {
        let width = table.columns.len() + usize::from(!table.row_content.is_empty());
        let text = renderer.language.text("table.empty", "No entries");
        write_empty_row(renderer.output(), width, text);
    }
    rows.try_for_each(|row| render_row(table, row, renderer))?;
    renderer.push_markup("</tbody>");
    Ok(())
}

fn write_empty_row(out: &mut String, column_count: usize, text: &str) {
    out.push_str("<tr class=\"sofie-empty\"><td colspan=\"");
    push_display(out, column_count);
    out.push_str("\">");
    push_escaped(out, text);
    out.push_str("</td></tr>");
}

fn render_row<'a>(
    table: &'a Table,
    row: Shown<'a>,
    renderer: &mut Renderer<'a>,
) -> Result<(), LocatedError> {
    renderer.push_markup("<tr>");
    for (index, column) in table.columns.iter().enumerate() {
        let link = table.link.as_ref().filter(|_| index == 0);
        write_cell(
            renderer,
            field_of(Some(row), column),
            link.map(|link| (link, row)),
        );
    }
    if !table.row_content.is_empty() {
        renderer.push_markup("<td class=\"sofie-actions\">");
        renderer.render_with_item(row, &table.row_content)?;
        renderer.push_markup("</td>");
    }
    renderer.push_markup("</tr>");
    Ok(())
}

fn write_cell<'a>(
    renderer: &mut Renderer<'a>,
    cell: Resolved<'_>,
    link: Option<(&'a Expression, Shown<'a>)>,
) {
    let amount = money_of(cell);
    renderer.push_markup(if amount.is_some() {
        "<td class=\"sofie-money\">"
    } else {
        "<td>"
    });
    if let Some((link, row)) = link {
        renderer.push_markup("<a");
        renderer.write_url_attribute_with_item("href", link, row);
        renderer.push_markup(">");
    }
    match (amount, cell) {
        (Some(amount), _) => renderer.write_money(amount),
        (None, Some(Shown::Instant(instant))) => renderer.write_instant(instant),
        (None, _) => write_escaped(renderer.output(), cell),
    }
    if link.is_some() {
        renderer.push_markup("</a>");
    }
    renderer.push_markup("</td>");
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sofie_core::show::Fields;
    use sofie_core::{Instant, Show, Shown};

    use super::parse_columns;
    use crate::Template;
    use crate::syntax::Position;

    fn render(source: &str, data: &serde_json::Value) -> String {
        Template::parse("t", source).unwrap().render(data).unwrap()
    }

    #[test]
    fn link_makes_the_first_cell_a_link_to_the_row() {
        let html = render(
            "<Table id=\"t\" link=\"/orders/{order.id}?from={data.from}\" of={data.rows} \
             columns=\"total name\" as=\"order\" />",
            &json!({"from": "a b", "rows": [
                {"id": 7, "name": "<x>", "total": {"minor": 5, "currency": "EUR"}},
            ]}),
        );
        assert!(
            html.contains(
                "<tr><td class=\"sofie-money\"><a href=\"/orders/7?from=a%20b\">0.05 EUR</a></td>\
                 <td>&lt;x&gt;</td></tr>"
            ),
            "{html}"
        );
    }

    struct Row {
        at: Instant,
        note: String,
    }

    impl Show for Row {
        fn show(&self) -> Shown<'_> {
            Shown::Record(self)
        }
    }

    impl Fields for Row {
        fn name(&self, index: usize) -> Option<&str> {
            ["at", "note"].get(index).copied()
        }

        fn get(&self, name: &str) -> Option<Shown<'_>> {
            match name {
                "at" => Some(self.at.show()),
                "note" => Some(self.note.show()),
                _ => None,
            }
        }
    }

    #[test]
    fn an_instant_cell_is_written_on_the_display_zone_s_wall_clock_and_text_stays_text() {
        let rows = vec![Row {
            at: Instant::parse("2026-10-11T22:30:00.5Z").unwrap(),
            note: "2026-10-11T22:30:00Z".to_owned(),
        }];
        let template = Template::parse("t", "<Table id=\"t\" of={data} columns=\"at note\" />");

        let html = template.unwrap().render(&rows).unwrap();

        assert!(
            html.contains("<td>2026-10-11 22:30 UTC</td><td>2026-10-11T22:30:00Z</td>"),
            "{html}"
        );
    }

    #[test]
    fn link_sends_an_unsafe_scheme_nowhere() {
        let html = render(
            "<Table id=\"t\" of={data.rows} columns=\"name\" as=\"row\" link={row.url} />",
            &json!({"rows": [{"name": "x", "url": "javascript:alert(1)"}]}),
        );
        assert!(html.contains("<td><a href=\"#\">x</a></td>"), "{html}");
    }

    #[test]
    fn link_needs_the_row_named() {
        let error = Template::parse(
            "t",
            "<Table id=\"t\" of={data.rows} columns=\"name\" link=\"/orders/{data.id}\" />",
        )
        .unwrap_err();
        assert!(error.to_string().contains("its `link` reads"), "{error}");
    }

    #[test]
    fn columns_must_be_field_names() {
        let position = Position { line: 1, column: 1 };
        assert!(parse_columns("  ", position).is_err());
        assert!(parse_columns("a <b>", position).is_err());
        assert_eq!(
            parse_columns(" a  b_c ", position).ok(),
            Some(vec!["a".to_owned(), "b_c".to_owned()])
        );
    }
}
