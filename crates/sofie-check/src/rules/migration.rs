use std::collections::HashMap;
use std::path::PathBuf;
use std::{fs, io};

use sofie_core::sql_lexer::{Lexed, Token, lex};
use sofie_model::Module;

use change::Change;

mod change;

/// What the migrations so far leave in the database: columns with their types, checks and indexes.
#[derive(Debug, Default)]
pub struct Schema {
    checks: Vec<Check>,
    indexes: Vec<Index>,
    columns: Vec<Column>,
}

#[derive(Debug)]
struct Index {
    table: String,
    name: String,
    columns: Vec<String>,
    unique: bool,
}

#[derive(Debug)]
struct Column {
    table: String,
    name: String,
    ty: Vec<Token>,
    nullable: bool,
}

/// Words ending a column's type in its definition.
const AFTER_TYPE: [&str; 11] = [
    "not",
    "null",
    "default",
    "check",
    "unique",
    "primary",
    "references",
    "constraint",
    "generated",
    "collate",
    "using",
];

#[derive(Debug)]
struct Check {
    table: String,
    name: String,
    column: Option<String>,
    tokens: Vec<Token>,
}

impl Check {
    fn covers(&self, table: &str, column: &str) -> bool {
        self.table == table
            && (self.column.as_deref() == Some(column)
                || self.tokens.iter().any(|token| is_named(token, column)))
    }

    fn literals(&self) -> Vec<&str> {
        self.tokens
            .iter()
            .filter_map(|token| match token {
                Token::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }
}

impl Schema {
    pub fn read(&mut self, migration: &Lexed) {
        for statement in migration.tokens.split(|token| *token == Token::Symbol(';')) {
            self.statement(statement);
        }
    }

    pub(super) fn column_type(&self, table: &str, column: &str) -> Option<&[Token]> {
        self.columns
            .iter()
            .find(|known| known.table == table && known.name == column)
            .map(|known| known.ty.as_slice())
    }

    /// The literals of each CHECK on the column, one list per CHECK, since
    /// only a single CHECK listing every value admits them all.
    #[must_use]
    pub fn check_literals(&self, table: &str, column: &str) -> Option<Vec<Vec<&str>>> {
        let covering: Vec<Vec<&str>> = self
            .checks
            .iter()
            .filter(|check| check.covers(table, column))
            .map(Check::literals)
            .collect();
        (!covering.is_empty()).then_some(covering)
    }

    #[must_use]
    pub fn knows_table(&self, table: &str) -> bool {
        self.columns.iter().any(|known| known.table == table)
    }

    #[must_use]
    pub fn has_column(&self, table: &str, column: &str) -> bool {
        self.column_type(table, column).is_some()
    }

    #[must_use]
    pub fn columns(&self, table: &str) -> Vec<&str> {
        self.columns
            .iter()
            .filter(|column| column.table == table)
            .map(|column| column.name.as_str())
            .collect()
    }

    #[must_use]
    pub fn is_unique(&self, table: &str, column: &str) -> bool {
        self.is_unique_over(table, &[column])
    }

    #[must_use]
    pub fn is_unique_over(&self, table: &str, columns: &[&str]) -> bool {
        self.indexes
            .iter()
            .any(|index| index.unique && index.table == table && index.columns == columns)
    }

    /// Under any of Postgres's names for the type: `int8` is a `bigint`, `varchar(20)` a `text`.
    #[must_use]
    pub fn has_type(&self, table: &str, column: &str, sql_type: &str) -> Option<bool> {
        let migrated = self.column_type(table, column)?;
        Some(type_family(migrated) == type_family(&lex(sql_type).tokens))
    }

    #[must_use]
    pub fn is_nullable(&self, table: &str, column: &str) -> Option<bool> {
        self.columns
            .iter()
            .find(|known| known.table == table && known.name == column)
            .map(|known| known.nullable)
    }

    /// An index leading with the column serves a lookup by it.
    #[must_use]
    pub fn is_indexed(&self, table: &str, column: &str) -> bool {
        self.indexes.iter().any(|index| {
            index.table == table && index.columns.first().is_some_and(|first| first == column)
        })
    }

    #[must_use]
    pub fn has_index(&self, name: &str) -> bool {
        self.indexes.iter().any(|index| index.name == name)
    }

    pub(super) fn statement(&mut self, tokens: &[Token]) {
        Change { schema: self }.statement(tokens);
    }
}

/// A column definition's type: `varchar(20)`, `character varying`, `numeric(10, 2)`.
pub(super) fn type_of(definition: &[Token]) -> &[Token] {
    let mut depth = 0_usize;
    let end = definition
        .iter()
        .position(|token| {
            match token {
                Token::Symbol('(') => depth += 1,
                Token::Symbol(')') => depth = depth.saturating_sub(1),
                _ => {}
            }
            depth == 0 && AFTER_TYPE.contains(&token.keyword())
        })
        .unwrap_or(definition.len());
    &definition[..end]
}

fn type_family(ty: &[Token]) -> String {
    let mut depth = 0_usize;
    let mut parts = Vec::new();
    for token in ty {
        match token {
            Token::Symbol('(') => depth += 1,
            Token::Symbol(')') => depth = depth.saturating_sub(1),
            Token::Symbol(symbol) if depth == 0 => parts.push(symbol.to_string()),
            _ if depth == 0 => parts.push(word(token).to_owned()),
            _ => {}
        }
    }
    let name = parts.join(" ");
    let family = match name.as_str() {
        "int8" | "bigserial" | "serial8" => "bigint",
        "int" | "int4" | "serial" | "serial4" => "integer",
        "bool" => "boolean",
        "varchar" | "character varying" | "char" | "character" | "bpchar" | "citext" => "text",
        "timestamp with time zone" => "timestamptz",
        "timestamp without time zone" => "timestamp",
        "float8" | "float" => "double precision",
        _ => return name,
    };
    family.to_owned()
}

/// An identifier's name, quoted or not, or a keyword.
pub(super) fn word(token: &Token) -> &str {
    match token {
        Token::Word(word) | Token::Quoted(word) => word,
        _ => "",
    }
}

pub(super) fn is_identifier(token: &Token) -> bool {
    matches!(token, Token::Word(_) | Token::Quoted(_))
}

fn is_named(token: &Token, name: &str) -> bool {
    is_identifier(token) && word(token) == name
}

pub(super) fn skip_words<'t>(tokens: &'t [Token], words: &[&str]) -> &'t [Token] {
    let skipped = tokens
        .iter()
        .take_while(|token| words.contains(&token.keyword()))
        .count();
    &tokens[skipped..]
}

pub(super) fn table_name(tokens: &[Token]) -> (String, &[Token]) {
    let tokens = skip_words(tokens, &["if", "not", "exists", "only"]);
    let mut end = 1;
    while tokens.get(end) == Some(&Token::Symbol('.')) {
        end += 2;
    }
    let name = tokens
        .get(..end.min(tokens.len()))
        .and_then(<[Token]>::last)
        .map_or("", word);
    (name.to_owned(), tokens.get(end..).unwrap_or_default())
}

pub(super) fn enclosed(tokens: &[Token]) -> &[Token] {
    let mut depth = 0_usize;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            Token::Symbol('(') => depth += 1,
            Token::Symbol(')') if depth == 1 => return &tokens[1..index],
            Token::Symbol(')') => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    tokens.get(1..).unwrap_or_default()
}

pub(super) fn split_top_level(tokens: &[Token]) -> Vec<&[Token]> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0_usize, 0);
    for (index, token) in tokens.iter().enumerate() {
        match token {
            Token::Symbol('(') => depth += 1,
            Token::Symbol(')') => depth = depth.saturating_sub(1),
            Token::Symbol(',') if depth == 0 => {
                parts.push(&tokens[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&tokens[start..]);
    parts
}

/// A unique expression of one column, such as `lower(email)`, keeps that column's values distinct too.
fn identifiers(tokens: &[Token]) -> Vec<String> {
    split_top_level(tokens)
        .into_iter()
        .filter_map(|element| {
            if element.get(1) != Some(&Token::Symbol('(')) {
                return element.first().map(word).map(str::to_owned);
            }
            let read: Vec<&str> = element
                .iter()
                .enumerate()
                .filter(|(index, token)| {
                    is_identifier(token) && element.get(index + 1) != Some(&Token::Symbol('('))
                })
                .map(|(_, token)| word(token))
                .collect();
            match read.as_slice() {
                [column] => Some((*column).to_owned()),
                _ => None,
            }
        })
        .collect()
}

pub(super) struct Migrations(HashMap<PathBuf, io::Result<Lexed>>);

impl Migrations {
    pub(super) fn read(modules: &[Module]) -> Self {
        let lexed = modules
            .iter()
            .flat_map(|module| &module.migrations)
            .map(|file| (file.clone(), fs::read_to_string(file).map(|sql| lex(&sql))))
            .collect();
        Self(lexed)
    }

    /// In the module's order, which later migrations build on.
    pub(super) fn of<'m>(
        &'m self,
        module: &'m Module,
    ) -> impl Iterator<Item = (&'m PathBuf, &'m io::Result<Lexed>)> {
        module
            .migrations
            .iter()
            .filter_map(|file| Some((file, self.0.get(file)?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(sql: &str) -> Schema {
        let mut constraints = Schema::default();
        constraints.read(&lex(sql));
        constraints
    }

    #[test]
    fn finds_checks_in_tables_columns_and_later_migrations() {
        let constraints = read(
            "CREATE TABLE product (\n  id uuid PRIMARY KEY,\n  name text NOT NULL CHECK (char_length(name) <= 120),\n\
             -- a comment; with a semicolon\n  price bigint,\n  CONSTRAINT positive CHECK (price > 0)\n);\n\
             ALTER TABLE \"product\" ADD COLUMN status text, ADD CHECK (status IN ('DRAFT', 'LIVE'));",
        );

        assert_eq!(
            constraints.check_literals("product", "name"),
            Some(vec![vec![]])
        );
        assert!(constraints.check_literals("product", "price").is_some());
        assert_eq!(
            constraints.check_literals("product", "status"),
            Some(vec![vec!["DRAFT", "LIVE"]])
        );
        assert_eq!(constraints.check_literals("product", "id"), None);
        assert_eq!(constraints.check_literals("order", "price"), None);
    }

    #[test]
    fn finds_unique_columns_indexes_and_constraints() {
        let constraints = read(
            "CREATE TABLE catalog.product (sku text UNIQUE, slug text, code text, a int, b int, UNIQUE (a, b));\n\
             CREATE UNIQUE INDEX IF NOT EXISTS product_slug ON ONLY product (slug) WHERE deleted_at IS NULL;\n\
             ALTER TABLE product ADD CONSTRAINT product_code UNIQUE (code);",
        );

        for column in ["sku", "slug", "code"] {
            assert!(constraints.is_unique("product", column), "{column}");
        }
        assert!(!constraints.is_unique("product", "a"));
    }

    #[test]
    fn each_check_lists_its_own_literals_and_a_dropped_one_lists_none() {
        let constraints = read(
            "CREATE TABLE line (status text CHECK (status <> 'CLOSED'), kind text, \
             CONSTRAINT kinds CHECK (kind IN ('PLAIN')));\n\
             ALTER TABLE line ADD CHECK (status IN ('OPEN'));\n\
             ALTER TABLE line DROP CONSTRAINT kinds, ADD CONSTRAINT kinds2 CHECK (kind IN ('GIFT'));\n\
             ALTER TABLE line DROP CONSTRAINT IF EXISTS line_status_check;",
        );

        assert_eq!(
            constraints.check_literals("line", "status"),
            Some(vec![vec!["OPEN"]])
        );
        assert_eq!(
            constraints.check_literals("line", "kind"),
            Some(vec![vec!["GIFT"]])
        );
    }

    #[test]
    fn a_renamed_column_keeps_its_checks_and_uniques_and_a_dropped_one_loses_them() {
        let constraints = read(
            "CREATE TABLE line (state text CHECK (state IN ('OPEN')), code text UNIQUE, note text CHECK (note <> ''));\n\
             ALTER TABLE line RENAME COLUMN state TO status;\n\
             ALTER TABLE line RENAME code TO sku;\n\
             ALTER TABLE line DROP COLUMN note;",
        );

        assert!(constraints.has_column("line", "status"));
        assert!(!constraints.has_column("line", "state"));
        assert_eq!(
            constraints.check_literals("line", "status"),
            Some(vec![vec!["OPEN"]])
        );
        assert!(constraints.is_unique("line", "sku"));
        assert!(!constraints.has_column("line", "note"));
        assert_eq!(constraints.check_literals("line", "note"), None);
    }

    #[test]
    fn an_index_serves_its_leading_column_until_the_column_is_dropped() {
        let schema = read(
            "CREATE TABLE line (id uuid, account uuid, kind text, note text);\n\
             CREATE INDEX CONCURRENTLY IF NOT EXISTS line_account ON line (account, kind);\n\
             CREATE INDEX line_note ON line (note);\n\
             ALTER TABLE line DROP COLUMN note;",
        );

        assert!(schema.is_indexed("line", "account"));
        assert!(!schema.is_indexed("line", "kind"));
        assert!(!schema.is_indexed("line", "note"));
        assert_eq!(schema.columns("line"), ["id", "account", "kind"]);
    }

    #[test]
    fn a_dropped_index_or_table_is_gone_and_a_renamed_table_keeps_its_schema() {
        let schema = read(
            "CREATE TABLE line (id uuid, account uuid, code text UNIQUE, kind text CHECK (kind IN ('A')));\n\
             CREATE INDEX line_account ON line (account);\n\
             CREATE TABLE note (id uuid);\n\
             DROP INDEX CONCURRENTLY IF EXISTS public.line_account;\n\
             ALTER TABLE line RENAME TO entry;\n\
             DROP TABLE IF EXISTS note CASCADE;",
        );

        assert!(!schema.is_indexed("entry", "account"));
        assert!(!schema.has_index("line_account"));
        assert!(!schema.knows_table("line"));
        assert_eq!(schema.columns("entry"), ["id", "account", "code", "kind"]);
        assert!(schema.is_unique("entry", "code"));
        assert!(schema.has_index("line_code_key"));
        assert_eq!(
            schema.check_literals("entry", "kind"),
            Some(vec![vec!["A"]])
        );
        assert!(!schema.knows_table("note"));
    }

    #[test]
    fn a_primary_key_serves_a_lookup_by_its_leading_column() {
        let schema = read(
            "CREATE TABLE member (account uuid, team uuid, PRIMARY KEY (account, team));\n\
             CREATE TABLE seat (number int PRIMARY KEY, CONSTRAINT seat_row UNIQUE (number));\n\
             ALTER TABLE seat DROP CONSTRAINT seat_row;",
        );

        assert!(schema.is_indexed("member", "account"));
        assert!(!schema.is_indexed("member", "team"));
        assert!(schema.is_unique("seat", "number"));
        assert!(!schema.has_index("seat_row"));
    }

    #[test]
    fn a_column_s_type_is_read_under_its_aliases_and_its_nullability_follows_alters() {
        let schema = read(
            "CREATE TABLE line (id uuid PRIMARY KEY, version int8 NOT NULL, note varchar(20), \
             at timestamp(3) with time zone, count integer, owner uuid, PRIMARY KEY (owner));\n\
             ALTER TABLE line ALTER COLUMN note SET NOT NULL, ALTER COLUMN version DROP NOT NULL;",
        );

        assert_eq!(schema.has_type("line", "version", "bigint"), Some(true));
        assert_eq!(schema.has_type("line", "note", "text"), Some(true));
        assert_eq!(schema.has_type("line", "at", "timestamptz"), Some(true));
        assert_eq!(schema.has_type("line", "count", "text"), Some(false));
        assert_eq!(schema.has_type("line", "gone", "text"), None);
        assert_eq!(schema.is_nullable("line", "id"), Some(false));
        assert_eq!(schema.is_nullable("line", "note"), Some(false));
        assert_eq!(schema.is_nullable("line", "version"), Some(true));
        assert_eq!(schema.is_nullable("line", "count"), Some(true));
        assert_eq!(schema.is_nullable("line", "owner"), Some(false));
    }

    #[test]
    fn a_quoted_column_named_like_a_keyword_is_a_column() {
        let constraints = read(
            "CREATE TABLE line (id uuid);\nALTER TABLE line ADD COLUMN \"check\" text, ADD \"unique\" text;",
        );

        assert!(constraints.has_column("line", "check"));
        assert!(constraints.has_column("line", "unique"));
        assert_eq!(constraints.check_literals("line", "id"), None);
    }

    #[test]
    fn a_unique_expression_of_one_column_keeps_that_column_unique() {
        let constraints = read(
            "CREATE TABLE account (email text, name text);\n\
             CREATE UNIQUE INDEX account_email ON account (lower(email));",
        );

        assert!(constraints.is_unique("account", "email"));
        assert!(!constraints.is_unique("account", "lower"));
    }
}
