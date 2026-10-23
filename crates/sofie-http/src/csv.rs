//! CSV out of a list a loader answers (`.document(Media::Csv)`) and into an
//! action's input, row by row (`CsvRows`), as RFC 4180 writes it.

use serde_json::{Map, Value};
use sofie_core::{Error, FieldError, Result, Shown};

use crate::{Area, RequestContext};
use sofie_db::{Input, Resolve};

/// A spreadsheet runs a cell starting with one of these as a formula, unless it is a number.
const FORMULA_STARTS: [char; 6] = ['=', '+', '-', '@', '\t', '\r'];

/// One row per record of the list, headed by the first record's field names.
pub(crate) fn render(shown: Shown<'_>) -> Result<String> {
    let Shown::List(items) = shown else {
        return Err(Error::internal_message(
            "a CSV route's loader answered no list of records: an export answers every row as a \
             `Vec`, not a page such as `Paginated` or `Listed`",
        ));
    };
    let mut csv = String::new();
    let mut names: Vec<&str> = Vec::new();
    for index in 0.. {
        let Some(item) = items.item(index) else { break };
        let Shown::Record(fields) = item else {
            return Err(Error::internal_message(
                "a CSV route's loader answered a list of something other than records",
            ));
        };
        if index == 0 {
            names = fields.names().collect();
            push_row(&mut csv, names.iter().map(|name| (*name).to_owned()));
        }
        push_row(
            &mut csv,
            names
                .iter()
                .map(|name| fields.get(name).map(cell).unwrap_or_default()),
        );
    }
    Ok(csv)
}

fn push_row(csv: &mut String, cells: impl Iterator<Item = String>) {
    for (index, cell) in cells.enumerate() {
        if index > 0 {
            csv.push(',');
        }
        push_cell(csv, &cell);
    }
    csv.push_str("\r\n");
}

fn push_cell(csv: &mut String, cell: &str) {
    let guarded = cell.starts_with(FORMULA_STARTS) && cell.parse::<f64>().is_err();
    if !guarded && !cell.contains([',', '"', '\n', '\r']) {
        csv.push_str(cell);
        return;
    }
    csv.push('"');
    if guarded {
        csv.push('\'');
    }
    csv.push_str(&cell.replace('"', "\"\""));
    csv.push('"');
}

fn cell(shown: Shown<'_>) -> String {
    match shown {
        Shown::Null => String::new(),
        Shown::Bool(flag) => flag.to_string(),
        Shown::Integer(number) => number.to_string(),
        Shown::Float(number) => number.to_string(),
        Shown::Number(number) => number.to_string(),
        Shown::Text(text) => text.to_owned(),
        Shown::Uuid(id) => id.to_string(),
        Shown::Instant(instant) => instant.to_string(),
        Shown::Formatted(value) => value.to_string(),
        Shown::List(items) => (0..)
            .map_while(|index| items.item(index))
            .map(cell)
            .collect::<Vec<_>>()
            .join("; "),
        Shown::Record(fields) => fields
            .names()
            .filter_map(|name| fields.get(name))
            .map(cell)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// The request body's CSV, each row read as its input is from a form; a row
/// the input refuses refuses them all, its errors named `<row>.<field>`.
pub struct CsvRows<I>(pub Vec<I>);

impl<A: Area, I: Input + Send + 'static> Resolve<RequestContext<A>> for CsvRows<I> {
    fn resolve(context: &mut RequestContext<A>) -> impl Future<Output = Result<Self>> + Send {
        let body = context.matched.body.take();
        std::future::ready(
            body.ok_or_else(|| Error::BadRequest("the request body was already read".to_owned()))
                .and_then(|body| {
                    let text = std::str::from_utf8(&body)
                        .map_err(|_| Error::BadRequest("the CSV is not UTF-8".to_owned()))?;
                    rows(text)
                }),
        )
    }
}

fn rows<I: Input>(text: &str) -> Result<CsvRows<I>> {
    let mut lines = parse(text)?.into_iter();
    let header = lines
        .next()
        .ok_or_else(|| Error::BadRequest("the CSV has no header row".to_owned()))?;
    let mut read = Vec::new();
    let mut refused = Vec::new();
    for (index, cells) in lines.enumerate() {
        let row = index + 2;
        if cells.len() != header.len() {
            return Err(Error::BadRequest(format!(
                "row {row} has {} cells; the header names {}",
                cells.len(),
                header.len()
            )));
        }
        let fields: Map<String, Value> = header
            .iter()
            .cloned()
            .zip(cells.into_iter().map(Value::String))
            .collect();
        match I::from_json(Value::Object(fields)) {
            Ok(input) => read.push(input),
            Err(Error::Invalid(errors)) => {
                refused.extend(errors.into_iter().map(|error| FieldError {
                    field: format!("{row}.{}", error.field),
                    ..error
                }));
            }
            Err(error) => return Err(error),
        }
    }
    if refused.is_empty() {
        Ok(CsvRows(read))
    } else {
        Err(Error::Invalid(refused))
    }
}

/// Fields quoted with `"` may hold commas, line breaks and `""`; blank lines are skipped.
fn parse(text: &str) -> Result<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut characters = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(character) = characters.next() {
        match (quoted, character) {
            (true, '"') if characters.peek() == Some(&'"') => {
                cell.push('"');
                characters.next();
            }
            (true, '"') => quoted = false,
            (false, '"') if cell.is_empty() => quoted = true,
            (false, ',') => row.push(std::mem::take(&mut cell)),
            (false, '\r') => {}
            (false, '\n') => end_row(&mut rows, &mut row, &mut cell),
            _ => cell.push(character),
        }
    }
    if quoted {
        return Err(Error::BadRequest(
            "the CSV ends inside a quoted cell".to_owned(),
        ));
    }
    end_row(&mut rows, &mut row, &mut cell);
    Ok(rows)
}

fn end_row(rows: &mut Vec<Vec<String>>, row: &mut Vec<String>, cell: &mut String) {
    row.push(std::mem::take(cell));
    let row = std::mem::take(row);
    if row.len() > 1 || !row[0].is_empty() {
        rows.push(row);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn writes_quoted_cells_and_defuses_formulas() {
        let list = json!([
            {"name": "Scarf, blue", "note": "said \"soft\"", "total": 12},
            {"name": "=HYPERLINK(\"x\")", "note": null, "total": -3},
        ]);

        let csv = render(sofie_core::Show::show(&list)).unwrap();

        assert_eq!(
            csv,
            "name,note,total\r\n\"Scarf, blue\",\"said \"\"soft\"\"\",12\r\n\
             \"'=HYPERLINK(\"\"x\"\")\",,-3\r\n"
        );
    }

    #[test]
    fn a_page_of_a_list_is_refused_with_what_to_answer_instead() {
        let page = json!({"items": [{"name": "Scarf"}], "page": 1, "size": 20, "has_next": false});

        let error = render(sofie_core::Show::show(&page))
            .unwrap_err()
            .to_string();

        assert!(error.contains("not a page"), "{error}");
    }

    #[test]
    fn reads_quoted_cells_line_breaks_and_a_byte_order_mark() {
        let rows =
            parse("\u{feff}name,note\r\n\"a, b\",\"two\nlines\"\n\n\"say \"\"hi\"\"\",\n").unwrap();

        assert_eq!(
            rows,
            [
                vec!["name", "note"],
                vec!["a, b", "two\nlines"],
                vec!["say \"hi\"", ""],
            ]
        );
        assert!(parse("name\n\"open").is_err());
    }
}
