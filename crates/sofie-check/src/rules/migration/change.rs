use sofie_core::sql_lexer::Token;

use super::{
    Check, Column, Index, Schema, enclosed, identifiers, is_named, skip_words, split_top_level,
    table_name, type_of, word,
};

/// A migration statement applied to the schema it changes.
pub(super) struct Change<'s> {
    pub(super) schema: &'s mut Schema,
}

impl Change<'_> {
    pub(super) fn statement(&mut self, tokens: &[Token]) {
        let words: Vec<&str> = tokens.iter().take(3).map(Token::keyword).collect();
        match words.as_slice() {
            ["create", "table", ..] => {
                let (table, rest) = table_name(&tokens[2..]);
                let body = rest
                    .iter()
                    .position(|token| *token == Token::Symbol('('))
                    .map_or(&[][..], |open| enclosed(&rest[open..]));
                for element in split_top_level(body) {
                    self.element(&table, element);
                }
            }
            ["alter", "table", ..] => {
                let (table, rest) = table_name(skip_words(&tokens[2..], &["if", "exists"]));
                for action in split_top_level(rest) {
                    self.alter(&table, action);
                }
            }
            ["create", "unique", "index"] => self.add_index(&tokens[3..], true),
            ["create", "index", ..] => self.add_index(&tokens[2..], false),
            ["drop", "index", ..] => {
                for name in dropped_names(&tokens[2..], &["concurrently", "if", "exists"]) {
                    self.schema.indexes.retain(|index| index.name != name);
                }
            }
            ["drop", "table", ..] => {
                for table in dropped_names(&tokens[2..], &["if", "exists"]) {
                    self.drop_table(&table);
                }
            }
            _ => {}
        }
    }

    fn alter(&mut self, table: &str, action: &[Token]) {
        let Some((verb, rest)) = action.split_first() else {
            return;
        };
        match verb.keyword() {
            "add" if rest.first().map(Token::keyword) == Some("column") => {
                let definition = skip_words(&rest[1..], &["if", "not", "exists"]);
                if let Some((name, definition)) = definition.split_first() {
                    self.column(table, word(name), definition);
                }
            }
            "add" => self.element(table, rest),
            "alter" => self.alter_column(table, skip_words(rest, &["column"])),
            "drop" => self.drop(table, rest),
            "rename" => self.rename(table, rest),
            _ => {}
        }
    }

    fn alter_column(&mut self, table: &str, rest: &[Token]) {
        let Some((name, change)) = rest.split_first() else {
            return;
        };
        let name = word(name);
        let Some(column) = self
            .schema
            .columns
            .iter_mut()
            .find(|column| column.table == table && column.name == name)
        else {
            return;
        };
        let words: Vec<&str> = change.iter().take(3).map(Token::keyword).collect();
        match words.as_slice() {
            ["set", "not", "null"] => column.nullable = false,
            ["drop", "not", "null"] => column.nullable = true,
            _ => {
                let change = skip_words(change, &["set", "data"]);
                if change.first().map(Token::keyword) == Some("type") {
                    column.ty = type_of(&change[1..]).to_vec();
                }
            }
        }
    }

    fn drop(&mut self, table: &str, rest: &[Token]) {
        let is_constraint = rest.first().map(Token::keyword) == Some("constraint");
        let rest = skip_words(rest, &["column", "constraint", "if", "exists"]);
        let Some(name) = rest.first().map(word) else {
            return;
        };
        if is_constraint {
            self.schema
                .checks
                .retain(|check| check.table != table || check.name != name);
            self.schema
                .indexes
                .retain(|index| index.table != table || index.name != name);
            return;
        }
        // Postgres drops the constraints and indexes involving a dropped column with it.
        self.schema
            .checks
            .retain(|check| !check.covers(table, name));
        self.schema.indexes.retain(|index| {
            index.table != table || !index.columns.iter().any(|column| column == name)
        });
        self.schema
            .columns
            .retain(|column| column.table != table || column.name != name);
    }

    /// `RENAME [COLUMN] a TO b` and `RENAME CONSTRAINT a TO b`; `RENAME TO` names the table anew.
    fn rename(&mut self, table: &str, rest: &[Token]) {
        let is_constraint = rest.first().map(Token::keyword) == Some("constraint");
        let rest = skip_words(rest, &["column", "constraint"]);
        if rest.first().map(Token::keyword) == Some("to") {
            if let Some(to) = rest.get(1).map(word) {
                self.rename_table(table, to);
            }
            return;
        }
        let (Some(from), Some("to"), Some(to)) = (
            rest.first().map(word),
            rest.get(1).map(Token::keyword),
            rest.get(2).map(word),
        ) else {
            return;
        };
        let (from, to) = (from.to_owned(), to.to_owned());
        if is_constraint {
            for check in self
                .schema
                .checks
                .iter_mut()
                .filter(|check| check.table == table)
            {
                if check.name == from {
                    check.name.clone_from(&to);
                }
            }
            return;
        }
        let renamed = |name: &mut String| {
            if *name == from {
                name.clone_from(&to);
            }
        };
        for column in self
            .schema
            .columns
            .iter_mut()
            .filter(|column| column.table == table)
        {
            renamed(&mut column.name);
        }
        for index in self
            .schema
            .indexes
            .iter_mut()
            .filter(|index| index.table == table)
        {
            index.columns.iter_mut().for_each(renamed);
        }
        for check in self
            .schema
            .checks
            .iter_mut()
            .filter(|check| check.table == table)
        {
            check.column.as_mut().map(renamed);
            for token in &mut check.tokens {
                if is_named(token, &from) {
                    *token = Token::Word(to.clone());
                }
            }
        }
    }

    /// A table renamed keeps its index and constraint names, as Postgres does.
    fn rename_table(&mut self, from: &str, to: &str) {
        let renamed = |table: &mut String| {
            if table == from {
                to.clone_into(table);
            }
        };
        self.schema
            .columns
            .iter_mut()
            .for_each(|column| renamed(&mut column.table));
        self.schema
            .checks
            .iter_mut()
            .for_each(|check| renamed(&mut check.table));
        self.schema
            .indexes
            .iter_mut()
            .for_each(|index| renamed(&mut index.table));
    }

    fn drop_table(&mut self, table: &str) {
        self.schema.columns.retain(|column| column.table != table);
        self.schema.checks.retain(|check| check.table != table);
        self.schema.indexes.retain(|index| index.table != table);
    }

    fn element(&mut self, table: &str, tokens: &[Token]) {
        let tokens = skip_words(tokens, &["if", "not", "exists"]);
        let (name, tokens) = match tokens.first().map(Token::keyword) {
            Some("constraint") => (tokens.get(1).map(word), tokens.get(2..).unwrap_or_default()),
            _ => (None, tokens),
        };
        let Some(first) = tokens.first() else {
            return;
        };
        match first.keyword() {
            "check" => self.add_check(table, name, None, &tokens[1..]),
            "unique" => {
                let columns = identifiers(enclosed(&tokens[1..]));
                let name = name.map_or_else(
                    || format!("{table}_{}_key", columns.join("_")),
                    str::to_owned,
                );
                self.push_index(table, name, columns, true);
            }
            "primary" => {
                let columns = identifiers(enclosed(skip_words(&tokens[1..], &["key"])));
                for column in self
                    .schema
                    .columns
                    .iter_mut()
                    .filter(|column| column.table == table && columns.contains(&column.name))
                {
                    column.nullable = false;
                }
                let name = name.map_or_else(|| format!("{table}_pkey"), str::to_owned);
                self.push_index(table, name, columns, true);
            }
            "foreign" | "exclude" => {}
            _ => self.column(table, word(first), &tokens[1..]),
        }
    }

    fn column(&mut self, table: &str, column: &str, definition: &[Token]) {
        let constraint = |keyword: &str| definition.iter().any(|token| token.keyword() == keyword);
        let not_null = definition
            .windows(2)
            .any(|pair| pair[0].keyword() == "not" && pair[1].keyword() == "null");
        self.schema.columns.push(Column {
            table: table.to_owned(),
            name: column.to_owned(),
            ty: type_of(definition).to_vec(),
            nullable: !not_null && !constraint("primary"),
        });
        if let Some(check) = definition
            .iter()
            .position(|token| token.keyword() == "check")
        {
            let name = definition[..check]
                .iter()
                .rposition(|token| token.keyword() == "constraint")
                .and_then(|at| definition.get(at + 1))
                .map(word);
            self.add_check(table, name, Some(column), &definition[check + 1..]);
        }
        if constraint("primary") {
            self.push_index(
                table,
                format!("{table}_pkey"),
                vec![column.to_owned()],
                true,
            );
        }
        if constraint("unique") {
            let name = format!("{table}_{column}_key");
            self.push_index(table, name, vec![column.to_owned()], true);
        }
    }

    fn push_index(&mut self, table: &str, name: String, columns: Vec<String>, unique: bool) {
        self.schema.indexes.push(Index {
            table: table.to_owned(),
            name,
            columns,
            unique,
        });
    }

    fn add_check(
        &mut self,
        table: &str,
        name: Option<&str>,
        column: Option<&str>,
        tokens: &[Token],
    ) {
        let name = name.map_or_else(|| self.default_check_name(table, tokens), str::to_owned);
        self.schema.checks.push(Check {
            table: table.to_owned(),
            name,
            column: column.map(str::to_owned),
            tokens: enclosed(tokens).to_vec(),
        });
    }

    /// Postgres's name for an unnamed CHECK: `<table>_<column>_check` when it
    /// reads one column, else `<table>_check`, numbered when taken.
    fn default_check_name(&self, table: &str, tokens: &[Token]) -> String {
        let mut read: Vec<&str> = self
            .schema
            .columns
            .iter()
            .filter(|column| {
                column.table == table
                    && enclosed(tokens)
                        .iter()
                        .any(|token| is_named(token, &column.name))
            })
            .map(|column| column.name.as_str())
            .collect();
        read.dedup();
        let base = match read.as_slice() {
            [column] => format!("{table}_{column}_check"),
            _ => format!("{table}_check"),
        };
        let taken = |name: &str| {
            self.schema
                .checks
                .iter()
                .any(|check| check.table == table && check.name == name)
        };
        let mut name = base.clone();
        let mut number = 0;
        while taken(&name) {
            number += 1;
            name = format!("{base}{number}");
        }
        name
    }

    /// `CREATE INDEX ON t (c)` lets Postgres name it `t_c_idx`.
    fn add_index(&mut self, tokens: &[Token], unique: bool) {
        let tokens = skip_words(tokens, &["concurrently", "if", "not", "exists"]);
        let Some(on) = tokens.iter().position(|token| token.keyword() == "on") else {
            return;
        };
        let (table, rest) = table_name(&tokens[on + 1..]);
        let columns = rest
            .iter()
            .position(|token| *token == Token::Symbol('('))
            .map_or_else(Vec::new, |open| identifiers(enclosed(&rest[open..])));
        let name = tokens[..on].last().map_or_else(
            || format!("{table}_{}_idx", columns.join("_")),
            |name| word(name).to_owned(),
        );
        self.push_index(&table, name, columns, unique);
    }
}

fn dropped_names(tokens: &[Token], options: &[&str]) -> Vec<String> {
    split_top_level(skip_words(tokens, options))
        .into_iter()
        .map(|name| table_name(name).0)
        .filter(|name| !name.is_empty())
        .collect()
}
