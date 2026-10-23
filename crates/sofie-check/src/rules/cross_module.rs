use std::path::Path;

use sofie_core::sql_lexer::{Lexed, Token, lex};
use sofie_model::Module;

use super::migration::{Migrations, is_identifier, skip_words, word};
use super::{Finding, Rule};

/// Schemas no module owns. `sofie` holds the framework's internals, which the
/// module's role is granted (`sofie.module_role`); `sofie_identity` is a module.
const SHARED_SCHEMAS: [&str; 5] = [
    "sofie",
    "pg_catalog",
    "information_schema",
    "public",
    "pg_temp",
];

const TABLE_KEYWORDS: [&str; 16] = [
    "join",
    "references",
    "into",
    "update",
    "table",
    "using",
    "truncate",
    "lock",
    "view",
    "sequence",
    "type",
    "domain",
    "index",
    "function",
    "procedure",
    "column",
];

/// The words before `SCHEMA` that name one: `DROP SCHEMA`, `GRANT … ON SCHEMA`,
/// `… IN SCHEMA`, `ALTER TABLE … SET SCHEMA`.
const SCHEMA_VERBS: [&str; 6] = ["drop", "create", "alter", "on", "in", "set"];

/// A migration runs with the module's schema as its search path, so only a
/// schema-qualified name can reach another module's tables.
pub(super) fn check_cross_module(modules: &[Module], migrations: &Migrations) -> Vec<Finding> {
    let module_schemas: Vec<&str> = modules
        .iter()
        .map(|module| module.krate.crate_name.as_str())
        .collect();
    let mut findings = Vec::new();
    for module in modules {
        let own = module.krate.crate_name.as_str();
        // `migration-lock` reports a migration it cannot read.
        for (file, migration) in migrations.of(module) {
            if let Ok(migration) = migration {
                findings.extend(
                    foreign_names(migration, own)
                        .into_iter()
                        .map(|reach| finding(file, migration, &reach, 1, own, &module_schemas)),
                );
            }
        }
        // Raw SQL in code runs with the module's schema as its search path too.
        for raw in &module.code.raw_sql {
            let Some((text, first_line)) = &raw.text else {
                continue;
            };
            let statement = lex(text);
            findings.extend(foreign_names(&statement, own).into_iter().map(|reach| {
                finding(
                    &raw.file,
                    &statement,
                    &reach,
                    *first_line,
                    own,
                    &module_schemas,
                )
            }));
        }
    }
    findings
}

fn finding(
    file: &Path,
    sql: &Lexed,
    reach: &Reach,
    first_line: usize,
    own: &str,
    module_schemas: &[&str],
) -> Finding {
    let schema = reach.schema.as_str();
    let reached = match &reach.object {
        Some(object) => format!("{schema}.{object}"),
        None => schema.to_owned(),
    };
    let owner = if module_schemas.contains(&schema) || schema == "sofie_identity" {
        format!("module `{schema}`")
    } else {
        format!("schema `{schema}`, which module `{own}` does not own")
    };
    Finding {
        rule: Rule::CrossModuleSql,
        file: file.to_path_buf(),
        line: first_line + sql.lines[reach.index] - 1,
        message: format!(
            "`{reached}` reaches into {owner}; no foreign key, join or query crosses modules"
        ),
        fix: "keep the other module's id as a plain column and read its data through its contract"
            .to_owned(),
    }
}

/// A schema a statement names: qualifying a table or a call, or on its own
/// (`DROP SCHEMA`, a `search_path`).
struct Reach {
    index: usize,
    schema: String,
    object: Option<String>,
}

impl Reach {
    fn qualified(tokens: &[Token], index: usize) -> Self {
        Self {
            index,
            schema: word(&tokens[index]).to_owned(),
            object: tokens.get(index + 2).map(|name| word(name).to_owned()),
        }
    }

    fn schema(index: usize, schema: &str) -> Self {
        Self {
            index,
            schema: schema.to_owned(),
            object: None,
        }
    }
}

fn foreign_names(migration: &Lexed, own: &str) -> Vec<Reach> {
    let mut reaches: Vec<Reach> = named_schemas(&migration.tokens)
        .into_iter()
        .filter(|reach| reach.schema != own && !SHARED_SCHEMAS.contains(&reach.schema.as_str()))
        .collect();
    reaches.sort_by_key(|reach| reach.index);
    reaches.dedup_by_key(|reach| reach.index);
    reaches
}

fn named_schemas(tokens: &[Token]) -> Vec<Reach> {
    let mut found = Vec::new();
    let mut open = Vec::new();
    let mut statement = 0;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            Token::Symbol('(') => open.push(index),
            Token::Symbol(')') => {
                open.pop();
            }
            Token::Symbol(';') => statement = index + 1,
            Token::Text(text) if text == "search_path" => {
                found.extend(configured_path(tokens, index));
            }
            Token::Word(_) | Token::Quoted(_) => {
                if is_qualified(tokens, index) && tokens.get(index + 3) == Some(&Token::Symbol('('))
                {
                    found.push(Reach::qualified(tokens, index));
                }
                let written = token.keyword();
                let before = &tokens[statement..index];
                if written == "search_path" {
                    found.extend(set_path(tokens, index + 1));
                } else if written == "schema"
                    && before
                        .last()
                        .is_some_and(|verb| SCHEMA_VERBS.contains(&verb.keyword()))
                {
                    found.extend(listed_schemas(tokens, index + 1));
                } else if !written.is_empty() && names_table(tokens, written, before, &open) {
                    found.extend(listed_tables(tokens, index + 1));
                }
            }
            _ => {}
        }
    }
    found
}

fn names_table(tokens: &[Token], keyword: &str, statement: &[Token], open: &[usize]) -> bool {
    match keyword {
        // `a IS DISTINCT FROM b` compares two values.
        "from" if statement.last().map(word) == Some("distinct") => false,
        // `extract(year FROM created_at)` names a column, not a table.
        "from" => open.last().is_none_or(|paren| {
            matches!(
                tokens.get(paren + 1).map(word),
                Some("select" | "with" | "delete" | "update" | "insert")
            )
        }),
        // `JOIN … ON a.id = b.id` is a condition; a view or function body has `AS`.
        "on" => {
            matches!(
                statement.first().map(word),
                Some("create" | "grant" | "revoke" | "comment")
            ) && !statement
                .iter()
                .any(|token| matches!(word(token), "as" | "join"))
        }
        _ => TABLE_KEYWORDS.contains(&keyword),
    }
}

fn listed_tables(tokens: &[Token], mut position: usize) -> Vec<Reach> {
    let mut reaches = Vec::new();
    loop {
        let rest = tokens.get(position..).unwrap_or_default();
        position += rest.len() - skip_words(rest, &["only", "if", "not", "exists"]).len();
        if is_qualified(tokens, position) {
            reaches.push(Reach::qualified(tokens, position));
            position += 2;
        } else if !tokens.get(position).is_some_and(is_identifier) {
            return reaches;
        }
        position += 1;
        let alias = tokens
            .get(position..)
            .unwrap_or_default()
            .iter()
            .take(2)
            .take_while(|token| is_identifier(token))
            .count();
        position += alias;
        if tokens.get(position) != Some(&Token::Symbol(',')) {
            return reaches;
        }
        position += 1;
    }
}

fn listed_schemas(tokens: &[Token], mut position: usize) -> Vec<Reach> {
    let rest = tokens.get(position..).unwrap_or_default();
    position += rest.len() - skip_words(rest, &["if", "not", "exists"]).len();
    let mut reaches = Vec::new();
    while let Some(name) = tokens.get(position).filter(|token| is_identifier(token)) {
        reaches.push(Reach::schema(position, word(name)));
        if tokens.get(position + 1) != Some(&Token::Symbol(',')) {
            break;
        }
        position += 2;
    }
    reaches
}

/// `SET search_path TO a, "b"` or `… = a`; `SET search_path FROM CURRENT` and
/// `TO DEFAULT` name none.
fn set_path(tokens: &[Token], position: usize) -> Vec<Reach> {
    let assigns = tokens.get(position) == Some(&Token::Symbol('='))
        || tokens.get(position).map(Token::keyword) == Some("to");
    if !assigns || tokens.get(position + 1).map(Token::keyword) == Some("default") {
        return Vec::new();
    }
    let mut reaches = Vec::new();
    let mut position = position + 1;
    while let Some(token) = tokens.get(position) {
        match token {
            Token::Word(name) | Token::Quoted(name) => reaches.push(Reach::schema(position, name)),
            Token::Text(path) => reaches.extend(path_entries(position, path)),
            Token::Symbol(_) => break,
        }
        if tokens.get(position + 1) != Some(&Token::Symbol(',')) {
            break;
        }
        position += 2;
    }
    reaches
}

/// `set_config('search_path', 'a, b', false)`.
fn configured_path(tokens: &[Token], index: usize) -> Vec<Reach> {
    match tokens.get(index + 1..index + 3) {
        Some([Token::Symbol(','), Token::Text(path)]) => path_entries(index + 2, path),
        _ => Vec::new(),
    }
}

/// `"$user"` names the login's own schema, never a module's.
fn path_entries(index: usize, path: &str) -> Vec<Reach> {
    path.split(',')
        .map(|entry| entry.trim().trim_matches('"'))
        .filter(|entry| !entry.is_empty() && *entry != "$user")
        .map(|entry| Reach::schema(index, entry))
        .collect()
}

fn is_qualified(tokens: &[Token], index: usize) -> bool {
    tokens.get(index).is_some_and(is_identifier)
        && tokens.get(index + 1) == Some(&Token::Symbol('.'))
        && tokens.get(index + 2).is_some_and(is_identifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn foreign(sql: &str) -> Vec<String> {
        let migration = lex(sql);
        foreign_names(&migration, "order")
            .into_iter()
            .map(|reach| format!("{}:{}", reach.schema, migration.lines[reach.index]))
            .collect()
    }

    #[test]
    fn finds_other_schemas_in_table_positions_and_calls() {
        let sql = "SELECT sofie.module_role('order');\n\
             CREATE TABLE IF NOT EXISTS \"order\" (account uuid REFERENCES sofie_identity.account (id));\n\
             CREATE VIEW priced AS SELECT o.id, extract(year FROM o.placed) FROM \"order\" o, catalog.product p\n\
             JOIN pricing.rule r ON r.product = p.id WHERE o.id IN (SELECT id FROM \"order\".\"order\");\n\
             CREATE INDEX order_account ON billing.invoice (account);\n\
             CREATE FUNCTION total() RETURNS bigint AS $$ SELECT tax.rate(1) $$ LANGUAGE sql;";

        assert_eq!(
            foreign(sql),
            [
                "sofie_identity:2",
                "catalog:3",
                "pricing:4",
                "billing:5",
                "tax:6"
            ]
        );
    }

    #[test]
    fn is_distinct_from_compares_and_names_no_table() {
        let sql = "CREATE FUNCTION touch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
             IF NEW.status IS DISTINCT FROM OLD.status THEN NEW.updated_at := now(); END IF; \
             RETURN NEW; END $$;\n\
             UPDATE \"order\" o SET total = p.total FROM price p WHERE o.total IS DISTINCT FROM p.total;";

        assert_eq!(foreign(sql), [] as [String; 0]);
    }

    #[test]
    fn finds_dropped_schemas_truncates_lists_nested_deletes_and_search_paths() {
        let sql = "DROP SCHEMA IF EXISTS billing, \"order\" CASCADE;\n\
             TRUNCATE \"order\", catalog.product RESTART IDENTITY;\n\
             DROP TABLE IF EXISTS legacy, pricing.rule;\n\
             WITH gone AS (DELETE FROM tax.rate WHERE true RETURNING id) SELECT count(*) FROM gone;\n\
             SET search_path TO shipping, public;\n\
             SELECT set_config('search_path', '\"$user\", stock', false);\n\
             ALTER TABLE \"order\" SET SCHEMA archive;\n\
             CREATE VIEW report.total AS SELECT 1;\n\
             SET search_path TO DEFAULT;\n\
             CREATE TABLE note (schema text, search_path text);";

        assert_eq!(
            foreign(sql),
            [
                "billing:1",
                "catalog:2",
                "pricing:3",
                "tax:4",
                "shipping:5",
                "stock:6",
                "archive:7",
                "report:8"
            ]
        );
    }
}
