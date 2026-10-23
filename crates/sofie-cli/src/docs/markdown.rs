use std::fmt::Write as _;
use std::path::Path;

pub(super) fn table<const COLUMNS: usize>(
    headers: [&str; COLUMNS],
    rows: &[[String; COLUMNS]],
) -> String {
    let mut text = row(&headers.map(str::to_owned));
    text.push_str(&row(&headers.map(|_| "---".to_owned())));
    for cells in rows {
        text.push_str(&row(cells));
    }
    text
}

fn row(cells: &[String]) -> String {
    let escaped: Vec<String> = cells
        .iter()
        .map(|cell| cell.replace('|', "\\|").replace('\n', " "))
        .collect();
    format!("| {} |\n", escaped.join(" | "))
}

pub(super) fn code(text: &str) -> String {
    if text.is_empty() {
        return "—".to_owned();
    }
    format!("`{text}`")
}

pub(super) fn codes<T: AsRef<str>>(items: &[T]) -> String {
    if items.is_empty() {
        return "—".to_owned();
    }
    let coded: Vec<String> = items.iter().map(|item| code(item.as_ref())).collect();
    coded.join(", ")
}

pub(super) fn mermaid(kind: &str, lines: &[String]) -> String {
    let mut text = format!("```mermaid\n{kind}\n");
    for line in lines {
        let _ = writeln!(text, "    {line}");
    }
    text.push_str("```\n");
    text
}

/// Text Mermaid shows as written: `#` and `;` end a statement or start an
/// entity code otherwise.
pub(super) fn mermaid_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '#' => escaped.push_str("#35;"),
            ';' => escaped.push_str("#59;"),
            '"' => escaped.push_str("#quot;"),
            other => escaped.push(other),
        }
    }
    escaped
}

pub(super) fn location(root: &Path, file: &Path, line: usize) -> String {
    let relative = file.strip_prefix(root).unwrap_or(file);
    format!("{}:{line}", relative.display())
}

#[cfg(test)]
mod tests {
    use super::{code, mermaid_text, table};

    #[test]
    fn escapes_cells_and_diagram_text() {
        let text = table(["A", "B"], &[["x|y".to_owned(), code("")]]);

        assert_eq!(text, "| A | B |\n| --- | --- |\n| x\\|y | — |\n");
        assert_eq!(mermaid_text("a;b#\"c\""), "a#59;b#35;#quot;c#quot;");
    }
}
