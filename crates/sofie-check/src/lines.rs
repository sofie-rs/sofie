use proc_macro2::{Span, TokenStream, TokenTree};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Code,
    Comment,
    Blank,
}

#[must_use]
pub fn classify_lines(text: &str, tokens: &TokenStream) -> Vec<LineKind> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut coverage = vec![Coverage::None; lines.len()];
    cover_tokens(tokens.clone(), &lines, &mut coverage);
    let mut fence = DocFence::default();
    lines
        .iter()
        .zip(coverage)
        .map(|(line, covered)| fence.classify(line, covered))
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Coverage {
    None,
    Doc,
    Code,
}

fn cover_tokens(tokens: TokenStream, lines: &[&str], coverage: &mut [Coverage]) {
    for token in tokens {
        match token {
            TokenTree::Group(group) if !is_doc_comment(group.span_open(), lines) => {
                cover(group.span_open(), Coverage::Code, coverage);
                cover(group.span_close(), Coverage::Code, coverage);
                cover_tokens(group.stream(), lines, coverage);
            }
            other => {
                let span = other.span();
                let kind = if is_doc_comment(span, lines) {
                    Coverage::Doc
                } else {
                    Coverage::Code
                };
                cover(span, kind, coverage);
            }
        }
    }
}

fn cover(span: Span, kind: Coverage, coverage: &mut [Coverage]) {
    for line in &mut coverage[span.start().line - 1..span.end().line] {
        *line = (*line).max(kind);
    }
}

/// A doc comment's `#[doc]` tokens start at `//` or `/*`; every other token starts elsewhere.
fn is_doc_comment(span: Span, lines: &[&str]) -> bool {
    let start = span.start();
    lines
        .get(start.line - 1)
        .and_then(|line| line.char_indices().nth(start.column))
        .is_some_and(|(index, _)| {
            let rest = &lines[start.line - 1][index..];
            rest.starts_with("//") || rest.starts_with("/*")
        })
}

/// Lines inside fenced code blocks of doc comments are doctests, so they count as code.
#[derive(Default)]
struct DocFence {
    is_open: bool,
}

impl DocFence {
    fn classify(&mut self, line: &str, covered: Coverage) -> LineKind {
        let text = meaningful_text(line);
        match covered {
            Coverage::Code => {
                self.is_open = false;
                LineKind::Code
            }
            Coverage::Doc if text.starts_with("```") => {
                self.is_open = !self.is_open;
                LineKind::Blank
            }
            _ if text.is_empty() => LineKind::Blank,
            Coverage::Doc if self.is_open => LineKind::Code,
            Coverage::Doc | Coverage::None => LineKind::Comment,
        }
    }
}

fn meaningful_text(line: &str) -> &str {
    line.trim()
        .trim_matches(|character| matches!(character, '*' | '/' | '!'))
        .trim()
}

#[cfg(test)]
mod tests {
    use super::{LineKind, classify_lines};
    use LineKind::{Blank, Code, Comment};

    fn classify(source: &str) -> Vec<LineKind> {
        classify_lines(source, &source.parse().expect("the sample tokenizes"))
    }

    #[test]
    fn plain_line_and_doc_comments_are_comments() {
        let source = "// one\n/// two\n//! three\nfn main() {}";
        assert_eq!(classify(source), [Comment, Comment, Comment, Code]);
    }

    #[test]
    fn delimiter_only_and_blank_lines_are_blank() {
        let source = "///\n\n   \n/*\n *\n */\n//////";
        assert_eq!(classify(source), [Blank; 7]);
    }

    #[test]
    fn code_with_trailing_comment_is_code() {
        assert_eq!(classify("let x = 1; // note"), [Code]);
    }

    #[test]
    fn block_comment_lines_are_comments() {
        let source = "/* first\n   second */\nlet x = /* inline */ 1;";
        assert_eq!(classify(source), [Comment, Comment, Code]);
    }

    #[test]
    fn nested_block_comment_ends_at_matching_close() {
        let source = "/* outer /* inner */\n still comment */\nlet x = 1;";
        assert_eq!(classify(source), [Comment, Comment, Code]);
    }

    #[test]
    fn comment_markers_inside_strings_are_code() {
        let source = "let a = \"// no\";\nlet b = \"/* no\";\nlet c = \"\\\" // no\";";
        assert_eq!(classify(source), [Code, Code, Code]);
    }

    #[test]
    fn multiline_string_lines_are_code() {
        let source = "let a = \"first\n// still string\n\n\";";
        assert_eq!(classify(source), [Code, Code, Code, Code]);
    }

    #[test]
    fn raw_strings_and_division_are_code() {
        let source = "let a = r##\"\"# /*\n\"##;\nlet b = 4 / 2;\n// after";
        assert_eq!(classify(source), [Code, Code, Code, Comment]);
    }

    #[test]
    fn a_doc_attribute_written_out_is_code() {
        let source = "#[doc = \"text\"]\nfn f() {}";
        assert_eq!(classify(source), [Code, Code]);
    }

    #[test]
    fn block_doc_comments_are_comments() {
        let source = "/**\n * Text.\n */\nfn f() {}\n/*! Inner. */";
        assert_eq!(classify(source), [Blank, Comment, Blank, Code, Comment]);
    }

    #[test]
    fn fenced_doc_lines_are_code() {
        let source = "/// Example:\n/// ```\n/// let x = 1;\n///\n/// ```\n/// Done.\nfn f() {}";
        assert_eq!(
            classify(source),
            [Comment, Blank, Code, Blank, Blank, Comment, Code]
        );
    }

    #[test]
    fn fences_in_plain_comments_are_ignored() {
        let source = "// ```\n// text\n// ```";
        assert_eq!(classify(source), [Comment, Comment, Comment]);
    }
}
