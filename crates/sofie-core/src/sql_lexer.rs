//! Postgres's lexical rules where they hide text: `--` and nested `/* */`
//! comments, `''` and `E'\''` escapes, `"quoted"` names and `$tag$` bodies.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    Word(String),
    /// A `"quoted"` identifier, never a keyword: `ADD COLUMN "check"` adds a column.
    Quoted(String),
    Text(String),
    Symbol(char),
}

impl Token {
    #[must_use]
    pub fn keyword(&self) -> &str {
        match self {
            Self::Word(word) => word,
            _ => "",
        }
    }
}

#[derive(Debug, Default)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub lines: Vec<usize>,
    pub comments: Vec<(usize, String)>,
}

/// A `$tag$` body is lexed on its own: its words stay visible, its `;` ends no statement.
#[must_use]
pub fn lex(sql: &str) -> Lexed {
    let characters: Vec<char> = sql.chars().collect();
    let mut lexed = Lexed::default();
    let mut line = 1;
    let mut index = 0;
    while index < characters.len() {
        let rest = &characters[index..];
        let (token, length) = match rest[0] {
            '-' if rest.get(1) == Some(&'-') => {
                let length = rest
                    .iter()
                    .position(|next| *next == '\n')
                    .unwrap_or(rest.len());
                let comment: String = rest[2..length].iter().collect();
                lexed.comments.push((line, comment.trim().to_owned()));
                (None, length)
            }
            '/' if rest.get(1) == Some(&'*') => (None, block_comment_length(rest)),
            '$' if index == 0 || !is_word_character(characters[index - 1]) => {
                match dollar_tag_length(rest) {
                    Some(tag) => {
                        let (body, length) = dollar_quoted(rest, tag);
                        lexed.append_body(lex(&body), line - 1);
                        (None, length)
                    }
                    None => (Some(Token::Symbol('$')), 1),
                }
            }
            '$' => (Some(Token::Symbol('$')), 1),
            quote @ ('\'' | '"') => {
                let escapes = quote == '\'' && is_escape_prefix(&lexed, &characters, index);
                if escapes {
                    lexed.tokens.pop();
                    lexed.lines.pop();
                }
                let (content, length) = quoted(rest, escapes);
                let token = if quote == '"' {
                    Token::Quoted(content)
                } else {
                    Token::Text(content)
                };
                (Some(token), length)
            }
            first if is_word_character(first) => {
                let length = rest
                    .iter()
                    .position(|next| !is_word_character(*next) && *next != '$')
                    .unwrap_or(rest.len());
                let word = rest[..length].iter().flat_map(|next| next.to_lowercase());
                (Some(Token::Word(word.collect())), length)
            }
            whitespace if whitespace.is_whitespace() => (None, 1),
            symbol => (Some(Token::Symbol(symbol)), 1),
        };
        if let Some(token) = token {
            lexed.tokens.push(token);
            lexed.lines.push(line);
        }
        line += rest[..length].iter().filter(|next| **next == '\n').count();
        index += length;
    }
    lexed
}

impl Lexed {
    fn append_body(&mut self, body: Lexed, lines_before: usize) {
        for (token, line) in body.tokens.into_iter().zip(body.lines) {
            if token != Token::Symbol(';') {
                self.tokens.push(token);
                self.lines.push(line + lines_before);
            }
        }
        let comments = body.comments.into_iter();
        self.comments
            .extend(comments.map(|(line, comment)| (line + lines_before, comment)));
    }
}

/// A `$` also continues a word, as `t$x$` names one table; it never starts one.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn block_comment_length(rest: &[char]) -> usize {
    let mut depth = 0;
    let mut index = 0;
    while index + 1 < rest.len() {
        match (rest[index], rest[index + 1]) {
            ('/', '*') => depth += 1,
            ('*', '/') => depth -= 1,
            _ => {
                index += 1;
                continue;
            }
        }
        index += 2;
        if depth == 0 {
            return index;
        }
    }
    rest.len()
}

/// `$$` or `$tag$`; a `$1` parameter is no tag.
fn dollar_tag_length(rest: &[char]) -> Option<usize> {
    let close = rest[1..].iter().position(|next| *next == '$')? + 1;
    let tag = &rest[1..close];
    let valid = tag.first().is_none_or(|first| !first.is_ascii_digit())
        && tag
            .iter()
            .all(|next| next.is_alphanumeric() || *next == '_');
    valid.then_some(close + 1)
}

fn dollar_quoted(rest: &[char], tag: usize) -> (String, usize) {
    let delimiter = &rest[..tag];
    let body = &rest[tag..];
    match body.windows(tag).position(|window| window == delimiter) {
        Some(end) => (body[..end].iter().collect(), tag + end + tag),
        None => (body.iter().collect(), rest.len()),
    }
}

fn is_escape_prefix(lexed: &Lexed, characters: &[char], index: usize) -> bool {
    index > 0
        && matches!(characters[index - 1], 'e' | 'E')
        && matches!(lexed.tokens.last(), Some(Token::Word(word)) if word == "e")
}

fn quoted(rest: &[char], escapes: bool) -> (String, usize) {
    let quote = rest[0];
    let mut content = String::new();
    let mut index = 1;
    while let Some(&next) = rest.get(index) {
        match next {
            '\\' if escapes => {
                content.extend(rest.get(index + 1));
                index += 2;
            }
            _ if next == quote && rest.get(index + 1) == Some(&quote) => {
                content.push(quote);
                index += 2;
            }
            _ if next == quote => return (content, index + 1),
            _ => {
                content.push(next);
                index += 1;
            }
        }
    }
    (content, rest.len())
}

#[cfg(test)]
mod tests {
    use super::{Token, lex};

    fn words(sql: &str) -> Vec<String> {
        lex(sql)
            .tokens
            .iter()
            .filter_map(|token| match token {
                Token::Word(word) => Some(word.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn block_comments_dollar_bodies_and_escapes_hide_no_statement() {
        let lexed = lex(
            "/* don't run /* nested */ at peak; */\nCREATE INDEX a ON t (x);\n\
             COMMENT ON TABLE t IS $c$ don't; $c$;\nSELECT E'it\\'s', 'it''s', $1;\n\
             CREATE INDEX b ON t (y);",
        );

        let texts: Vec<&str> = lexed
            .tokens
            .iter()
            .filter_map(|token| match token {
                Token::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["t; ", "it's", "it's"]);
        let create = |name: &str| {
            let word = Token::Word(name.to_owned());
            let at = lexed.tokens.iter().position(|token| *token == word);
            lexed.lines[at.unwrap()]
        };
        assert_eq!((create("a"), create("b")), (2, 5));
        let statements = lexed.tokens.split(|token| *token == Token::Symbol(';'));
        assert_eq!(statements.count(), 5);
        assert_eq!(
            words("/* ' */ CREATE INDEX a ON t (x);"),
            ["create", "index", "a", "on", "t", "x"]
        );
    }

    fn texts(sql: &str) -> Vec<String> {
        lex(sql)
            .tokens
            .iter()
            .filter_map(|token| match token {
                Token::Text(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_dollar_inside_a_word_opens_no_body() {
        assert_eq!(
            words("CREATE TABLE t$x$ (a int); CREATE INDEX b ON t (y);"),
            [
                "create", "table", "t$x$", "a", "int", "create", "index", "b", "on", "t", "y"
            ]
        );
        assert_eq!(words("SELECT 'a'$q$b$q$"), ["select", "b"]);
    }

    #[test]
    fn an_unterminated_quote_or_comment_ends_with_the_text() {
        assert_eq!(texts("SELECT 'abc"), ["abc"]);
        assert_eq!(
            words("SELECT 1 /* never closed; CREATE INDEX"),
            ["select", "1"]
        );
        assert_eq!(
            words("SELECT $a$ never closed; CREATE INDEX"),
            ["select", "never", "closed", "create", "index"]
        );
    }

    #[test]
    fn unicode_and_backslash_escaped_strings_end_where_postgres_ends_them() {
        let unicode = lex("SELECT U&'d\\0061t\\'; CREATE INDEX b ON t (y)");
        let backslash = lex("SELECT E'\\\\'; CREATE INDEX b ON t (y)");

        for lexed in [&unicode, &backslash] {
            let statements = lexed.tokens.split(|token| *token == Token::Symbol(';'));
            assert_eq!(statements.count(), 2, "{lexed:?}");
        }
        assert!(
            unicode
                .tokens
                .contains(&Token::Text("d\\0061t\\".to_owned()))
        );
        assert!(backslash.tokens.contains(&Token::Text("\\".to_owned())));
    }

    #[test]
    fn crlf_lines_are_counted_once() {
        let lexed = lex("SELECT 1;\r\n-- note\r\nCREATE INDEX b ON t (y);\r\n");

        let create = lexed
            .tokens
            .iter()
            .position(|token| *token == Token::Word("create".to_owned()))
            .unwrap();
        assert_eq!(lexed.lines[create], 3);
        assert_eq!(lexed.comments, [(2, "note".to_owned())]);
    }

    #[test]
    fn quoted_names_and_line_comments_hold_no_word() {
        let lexed = lex("ALTER TABLE t ADD \"Check\" text; -- Not now; later\nSELECT 1");

        assert!(lexed.tokens.contains(&Token::Quoted("Check".to_owned())));
        assert_eq!(lexed.comments, [(1, "Not now; later".to_owned())]);
        assert_eq!(
            words("ALTER TABLE t ADD \"Check\" text; -- Not now\nSELECT 1"),
            ["alter", "table", "t", "add", "text", "select", "1"]
        );
    }
}
