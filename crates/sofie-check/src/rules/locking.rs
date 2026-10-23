//! A migration runs while the previous version serves, so a statement holding a table's exclusive lock stops every request touching it.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use sofie_core::sql_lexer::{Lexed, Token, lex};
use sofie_model::naming::{behavior_columns, snake_case};
use sofie_model::{EnumDeclaration, FieldDeclaration, ModelError, Module};

use super::allowances::{parse_entries, read_optional};
use super::migration::{
    Migrations, Schema, enclosed, skip_words, split_top_level, table_name, type_of, word,
};
use super::{Finding, Rule};

const ALLOW: &str = "sofie:allow migration-lock:";

/// Exceptions for migrations already applied, whose text can no longer change, and for small tables.
const ALLOW_FILE: &str = "migration-allow.toml";

/// Functions a default calls anew for every row, so adding a column with
/// one fills each existing row while the table is locked.
const VOLATILE: [&str; 10] = [
    "random",
    "gen_random_uuid",
    "uuidv4",
    "uuidv7",
    "uuid_generate_v1",
    "uuid_generate_v1mc",
    "uuid_generate_v4",
    "clock_timestamp",
    "timeofday",
    "nextval",
];

const SERIAL_TYPES: [&str; 6] = [
    "serial",
    "bigserial",
    "smallserial",
    "serial2",
    "serial4",
    "serial8",
];

pub(super) fn check_locking(modules: &[Module], migrations: &Migrations) -> Vec<Finding> {
    modules
        .iter()
        .flat_map(|module| check_module(module, migrations))
        .collect()
}

fn check_module(module: &Module, migrations: &Migrations) -> Vec<Finding> {
    let used = UsedColumns::of(module);
    let allow_file = module.krate.path.join("migrations").join(ALLOW_FILE);
    let mut findings = Vec::new();
    let mut exceptions = Exceptions::read(&allow_file).unwrap_or_else(|error| {
        findings.push(Finding {
            rule: Rule::MigrationLock,
            file: error.file,
            line: error.line,
            message: error.message,
            fix: "write `[[statement]]` entries of `file`, `statement` and `reason` strings, \
                  or `[[table]]` entries of `name` and `reason`"
                .to_owned(),
        });
        Exceptions::default()
    });
    let mut schema = Schema::default();
    for (file, migration) in migrations.of(module) {
        match migration {
            Ok(migration) => findings.extend(check_migration(
                file,
                migration,
                &used,
                &mut schema,
                &mut exceptions,
            )),
            // The model lists the files on disk; one gone since has no
            // statement to hold, and the wiring embedding it reports it.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => findings.push(Finding {
                rule: Rule::MigrationLock,
                file: file.clone(),
                line: 1,
                message: format!("the migration cannot be read: {error}"),
                fix: "make the file readable UTF-8 text".to_owned(),
            }),
        }
    }
    findings.extend(exceptions.stale());
    findings
}

/// `migration-allow.toml`: statements the rule keeps, each named by its
/// migration's file name and its beginning, compared token by token, or every
/// lock on a table small enough to lock. A table entry keeps only locks: a
/// change breaking the running version breaks it on a small table too.
///
/// ```toml
/// [[statement]]
/// file = "0002_index.sql"
/// statement = "CREATE INDEX order_placed_at"
/// reason = "Why the lock is acceptable."
///
/// [[table]]
/// name = "country"
/// reason = "Why locking it is acceptable."
/// ```
#[derive(Default)]
struct Exceptions {
    allow_file: PathBuf,
    entries: Vec<Exception>,
}

struct Exception {
    kept: Kept,
    line: usize,
    matched: bool,
}

enum Kept {
    Statement { file: String, tokens: Vec<Token> },
    Table(String),
}

impl Exceptions {
    fn read(allow_file: &Path) -> Result<Self, ModelError> {
        let text = read_optional(allow_file)?.unwrap_or_default();
        Self::parse(allow_file, &text)
    }

    fn parse(allow_file: &Path, text: &str) -> Result<Self, ModelError> {
        let entries = parse_entries(
            allow_file,
            text,
            &["statement", "table"],
            &["file", "statement", "name"],
        )?
        .into_iter()
        .map(|entry| {
            let kept = if entry.kind == "table" {
                Kept::Table(entry.required(allow_file, "name")?.to_lowercase())
            } else {
                Kept::Statement {
                    file: entry.required(allow_file, "file")?,
                    tokens: lex(&entry.required(allow_file, "statement")?).tokens,
                }
            };
            Ok(Exception {
                kept,
                line: entry.line,
                matched: false,
            })
        })
        .collect::<Result<_, ModelError>>()?;
        Ok(Self {
            allow_file: allow_file.to_path_buf(),
            entries,
        })
    }

    fn allows(&mut self, migration: &Path, tokens: &[Token], locks: &[Lock]) -> bool {
        let name = migration.file_name().and_then(|name| name.to_str());
        let statement = self.entries.iter_mut().find(|exception| {
            matches!(&exception.kept, Kept::Statement { file, tokens: kept }
                if name == Some(file.as_str()) && !kept.is_empty() && tokens.starts_with(kept))
        });
        if let Some(exception) = statement {
            exception.matched = true;
            return true;
        }
        let tables: Option<Vec<&str>> = locks.iter().map(Lock::locked_table).collect();
        let Some(tables) = tables else {
            return false;
        };
        let kept = |exception: &Exception, table: &str| matches!(&exception.kept, Kept::Table(name) if name == table);
        if !tables
            .iter()
            .all(|table| self.entries.iter().any(|exception| kept(exception, table)))
        {
            return false;
        }
        for exception in &mut self.entries {
            if tables.iter().any(|table| kept(exception, table)) {
                exception.matched = true;
            }
        }
        true
    }

    /// An entry keeping no refused statement must go, or it would keep a
    /// future one nobody reviewed.
    fn stale(self) -> Vec<Finding> {
        let allow_file = self.allow_file;
        self.entries
            .into_iter()
            .filter(|exception| !exception.matched)
            .map(|exception| {
                let (message, fix) = match exception.kept {
                    Kept::Statement { file, .. } => (
                        format!("this entry keeps no statement of `{file}` that `migration-lock` refuses"),
                        "delete this entry, or correct its `file` and `statement`",
                    ),
                    Kept::Table(table) => (
                        format!("this entry keeps no lock on `{table}` that `migration-lock` refuses"),
                        "delete this entry, or correct its `name`",
                    ),
                };
                Finding {
                    rule: Rule::StaleAllowance,
                    file: allow_file.clone(),
                    line: exception.line,
                    message,
                    fix: fix.to_owned(),
                }
            })
            .collect()
    }
}

struct Statement<'a> {
    tokens: &'a [Token],
    line: usize,
    comment_lines: (usize, usize),
}

fn check_migration(
    file: &Path,
    lexed: &Lexed,
    used: &UsedColumns,
    schema: &mut Schema,
    exceptions: &mut Exceptions,
) -> Vec<Finding> {
    let statements = statements(lexed);
    let created: HashSet<String> = statements
        .iter()
        .filter(|statement| starts_with(statement.tokens, &["create", "table"]))
        .map(|statement| table_name(&statement.tokens[2..]).0)
        .collect();
    let file_holds_one_statement = statements.len() == 1;
    let mut findings = Vec::new();
    for statement in &statements {
        let mut locks = statement_locks(statement.tokens, &created, used, schema);
        schema.statement(statement.tokens);
        if !file_holds_one_statement && has_word(statement.tokens, "concurrently") {
            locks.push(Lock::ConcurrentNotAlone);
        }
        if locks.is_empty()
            || is_allowed(lexed, statement)
            || exceptions.allows(file, statement.tokens, &locks)
        {
            continue;
        }
        findings.extend(
            locks
                .into_iter()
                .map(|lock| lock.finding(file, statement.line)),
        );
    }
    findings
}

fn statements(lexed: &Lexed) -> Vec<Statement<'_>> {
    let mut statements = Vec::new();
    let (mut start, mut previous_end) = (0, 0);
    for (index, token) in lexed.tokens.iter().enumerate() {
        if *token != Token::Symbol(';') {
            continue;
        }
        if index > start {
            statements.push(Statement {
                tokens: &lexed.tokens[start..index],
                line: lexed.lines[start],
                comment_lines: (previous_end + 1, lexed.lines[index]),
            });
        }
        previous_end = lexed.lines[index];
        start = index + 1;
    }
    if let Some(last) = lexed.lines.last().filter(|_| start < lexed.tokens.len()) {
        statements.push(Statement {
            tokens: &lexed.tokens[start..],
            line: lexed.lines[start],
            comment_lines: (previous_end + 1, *last),
        });
    }
    statements
}

fn is_allowed(lexed: &Lexed, statement: &Statement) -> bool {
    let (first, last) = statement.comment_lines;
    lexed.comments.iter().any(|(line, text)| {
        (first..=last).contains(line)
            && text
                .strip_prefix(ALLOW)
                .is_some_and(|reason| !reason.trim().is_empty())
    })
}

fn statement_locks(
    tokens: &[Token],
    created: &HashSet<String>,
    used: &UsedColumns,
    schema: &Schema,
) -> Vec<Lock> {
    if starts_with(tokens, &["alter", "table"]) {
        let (table, rest) = table_name(skip_words(&tokens[2..], &["if", "exists"]));
        if created.contains(&table) {
            return Vec::new();
        }
        return split_top_level(rest)
            .into_iter()
            .filter_map(|action| action_lock(&table, action, used, schema))
            .collect();
    }
    let index = match tokens.get(1).map(word) {
        Some("unique") => &tokens[2..],
        _ => tokens.get(1..).unwrap_or_default(),
    };
    if starts_with(tokens, &["create"]) && starts_with(index, &["index"]) {
        return index_lock(index, created).into_iter().collect();
    }
    if let Some(locks) = removal_locks(tokens, created, used) {
        return locks;
    }
    maintenance_lock(tokens).into_iter().collect()
}

fn removal_locks(
    tokens: &[Token],
    created: &HashSet<String>,
    used: &UsedColumns,
) -> Option<Vec<Lock>> {
    let (truncates, names) = if starts_with(tokens, &["drop", "table"]) {
        (false, skip_words(&tokens[2..], &["if", "exists"]))
    } else if starts_with(tokens, &["truncate"]) {
        (true, skip_words(&tokens[1..], &["table"]))
    } else {
        return None;
    };
    let locks = split_top_level(names)
        .into_iter()
        .map(|entry| table_name(entry).0)
        .filter(|table| !created.contains(table) && used.declares(table))
        .map(|table| {
            if truncates {
                Lock::TruncateUsed { table }
            } else {
                Lock::DropUsedTable { table }
            }
        })
        .collect();
    Some(locks)
}

fn maintenance_lock(tokens: &[Token]) -> Option<Lock> {
    let concurrent = has_word(tokens, "concurrently");
    let statement = match tokens.first().map(word)? {
        "drop" if starts_with(&tokens[1..], &["index"]) && !concurrent => "DROP INDEX",
        "reindex" if !concurrent => "REINDEX",
        "vacuum" if has_word(tokens, "full") => "VACUUM FULL",
        "cluster" => "CLUSTER",
        _ => return None,
    };
    Some(Lock::Maintenance { statement })
}

fn index_lock(index: &[Token], created: &HashSet<String>) -> Option<Lock> {
    if index.get(1).map(word) == Some("concurrently") {
        return None;
    }
    let on = index.iter().position(|token| word(token) == "on")?;
    let (table, _) = table_name(&index[on + 1..]);
    (!created.contains(&table)).then_some(Lock::Index { table })
}

fn action_lock(table: &str, action: &[Token], used: &UsedColumns, schema: &Schema) -> Option<Lock> {
    let (verb, rest) = action.split_first()?;
    let is_column = rest.first().map(Token::keyword) == Some("column");
    let rest = skip_words(rest, &["column"]);
    match verb.keyword() {
        "add" if is_column => added_column_lock(table, skip_words(rest, &["if", "not", "exists"])),
        "add" => constraint_lock(table, rest)
            .or_else(|| added_column_lock(table, skip_words(rest, &["if", "not", "exists"]))),
        "rename" if !is_column && starts_with(rest, &["constraint"]) => None,
        "rename" if !is_column && starts_with(rest, &["to"]) => Some(Lock::RenameTable {
            table: table.to_owned(),
        }),
        "rename" => {
            let column = word(rest.first()?);
            (rest.get(1).map(Token::keyword) == Some("to")).then(|| Lock::RenameColumn {
                table: table.to_owned(),
                column: column.to_owned(),
            })
        }
        "set" if starts_with(rest, &["tablespace"]) => Some(Lock::Maintenance {
            statement: "SET TABLESPACE",
        }),
        "alter" => altered_column_lock(table, rest, schema),
        "drop" => {
            let rest = skip_words(rest, &["if", "exists"]);
            let column = word(rest.first()?);
            used.uses(table, column).then(|| Lock::DropUsed {
                table: table.to_owned(),
                column: column.to_owned(),
            })
        }
        _ => None,
    }
}

fn constraint_lock(table: &str, rest: &[Token]) -> Option<Lock> {
    let kind = if starts_with(rest, &["constraint"]) {
        rest.get(2..)?
    } else {
        rest
    };
    let table = table.to_owned();
    match kind.first().map(Token::keyword)? {
        "foreign" | "check" if !has_sequence(kind, &["not", "valid"]) => {
            Some(Lock::UnvalidatedConstraint { table })
        }
        "primary" | "unique" if !has_sequence(kind, &["using", "index"]) => {
            Some(Lock::KeyIndex { table })
        }
        _ => None,
    }
}

fn added_column_lock(table: &str, definition: &[Token]) -> Option<Lock> {
    let (name, rest) = definition.split_first()?;
    if matches!(
        name.keyword(),
        "constraint" | "check" | "unique" | "primary" | "foreign" | "exclude"
    ) {
        return None;
    }
    let column = word(name);
    let rewrites = rest
        .first()
        .is_some_and(|ty| SERIAL_TYPES.contains(&word(ty)))
        || has_word(rest, "identity")
        || has_word(rest, "stored")
        || calls_volatile(rest);
    let (table, column) = (table.to_owned(), column.to_owned());
    if rewrites {
        return Some(Lock::Rewrite { table, column });
    }
    if has_word(rest, "unique") || has_sequence(rest, &["primary", "key"]) {
        return Some(Lock::KeyIndex { table });
    }
    (has_sequence(rest, &["not", "null"]) && !has_word(rest, "default"))
        .then_some(Lock::NotNullWithoutDefault { table, column })
}

fn altered_column_lock(table: &str, rest: &[Token], schema: &Schema) -> Option<Lock> {
    let (name, change) = rest.split_first()?;
    let new_type = skip_words(change, &["set", "data"]);
    if starts_with(new_type, &["type"]) {
        let old = schema.column_type(table, word(name));
        if !has_word(new_type, "using")
            && old.is_some_and(|old| widens(old, type_of(&new_type[1..])))
        {
            return None;
        }
    }
    let (table, column) = (table.to_owned(), word(name).to_owned());
    if starts_with(new_type, &["type"]) {
        return Some(Lock::ColumnType { table, column });
    }
    starts_with(change, &["set", "not", "null"]).then_some(Lock::SetNotNull { table, column })
}

/// Postgres changes these types without rewriting the table or rechecking a row:
/// lengthening a `varchar` or `varbit`, unbounding one or making it `text`, and
/// raising a `numeric`'s precision at the same scale.
fn widens(old: &[Token], new: &[Token]) -> bool {
    let (Some(old), Some(new)) = (SqlType::read(old), SqlType::read(new)) else {
        return false;
    };
    if old == new {
        return true;
    }
    match (old.name.as_str(), new.name.as_str()) {
        ("varchar" | "text", "text") | ("text", "varchar") => new.sizes.is_empty(),
        ("varchar", "varchar") | ("varbit", "varbit") => match (&old.sizes[..], &new.sizes[..]) {
            (_, []) => true,
            ([old], [new]) => new >= old,
            _ => false,
        },
        ("numeric", "numeric") => match (&old.sizes[..], &new.sizes[..]) {
            (_, []) => true,
            ([old_precision, old_scale], [new_precision, new_scale]) => {
                new_precision >= old_precision && new_scale == old_scale
            }
            ([old_precision], [new_precision]) => new_precision >= old_precision,
            _ => false,
        },
        _ => false,
    }
}

#[derive(PartialEq, Eq)]
struct SqlType {
    name: String,
    sizes: Vec<u32>,
}

impl SqlType {
    fn read(tokens: &[Token]) -> Option<Self> {
        let open = tokens
            .iter()
            .position(|token| *token == Token::Symbol('('))
            .unwrap_or(tokens.len());
        let words: Vec<&str> = tokens[..open].iter().map(Token::keyword).collect();
        if words.is_empty() || words.contains(&"") {
            return None;
        }
        let name = match words.join(" ").as_str() {
            "character varying" | "varchar" => "varchar".to_owned(),
            "bit varying" | "varbit" => "varbit".to_owned(),
            "decimal" | "numeric" => "numeric".to_owned(),
            other => other.to_owned(),
        };
        let sizes = if open < tokens.len() {
            let inside = enclosed(&tokens[open..]);
            split_top_level(inside)
                .into_iter()
                .map(|size| match size {
                    [size] => size.keyword().parse().ok(),
                    _ => None,
                })
                .collect::<Option<Vec<u32>>>()?
        } else {
            Vec::new()
        };
        Some(Self { name, sizes })
    }
}

fn calls_volatile(tokens: &[Token]) -> bool {
    tokens
        .windows(2)
        .any(|pair| VOLATILE.contains(&word(&pair[0])) && pair[1] == Token::Symbol('('))
}

fn starts_with(tokens: &[Token], words: &[&str]) -> bool {
    tokens.len() >= words.len() && tokens.iter().zip(words).all(|(token, w)| word(token) == *w)
}

fn has_word(tokens: &[Token], expected: &str) -> bool {
    tokens.iter().any(|token| word(token) == expected)
}

fn has_sequence(tokens: &[Token], words: &[&str]) -> bool {
    (0..tokens.len()).any(|start| starts_with(&tokens[start..], words))
}

/// Value types storing more than one column, as `<field><suffix>`, from their `COLUMNS`.
const SUFFIXED: [(&str, &[&str]); 5] = [
    ("Money", &["", "_currency"]),
    ("Measurement", &["", "_unit"]),
    ("LocalDateTime", &["", "_zone"]),
    ("Coordinate", &["_latitude", "_longitude"]),
    ("PhoneNumber", &["_code", "_number"]),
];

/// Types stored in the one column named after their field.
const SINGLE: [&str; 29] = [
    "bool",
    "i8",
    "i16",
    "i32",
    "i64",
    "u8",
    "u16",
    "u32",
    "u64",
    "f32",
    "f64",
    "String",
    "Uuid",
    "Id",
    "Timestamp",
    "Date",
    "Time",
    "Email",
    "Url",
    "Slug",
    "Locale",
    "Currency",
    "TimeZone",
    "TimeOfDay",
    "Weekday",
    "Decimal",
    "Percentage",
    "PersonName",
    "Json",
];

enum UsedColumn {
    Exact(String),
    /// A field of a type the model cannot see into, which may store `<field>_<part>` columns.
    Prefix(String),
}

impl UsedColumn {
    fn of(field: &FieldDeclaration, enums: &[EnumDeclaration]) -> Vec<Self> {
        let mut ty = &field.ty;
        while matches!(ty.name.as_str(), "Option" | "Secret") {
            let Some(inner) = ty.arguments.first() else {
                break;
            };
            ty = inner;
        }
        let name = ty.name.as_str();
        if let Some((_, suffixes)) = SUFFIXED.iter().find(|(stored, _)| *stored == name) {
            return suffixes
                .iter()
                .map(|suffix| Self::Exact(format!("{}{suffix}", field.name)))
                .collect();
        }
        let single = SINGLE.contains(&name)
            || ty.record().is_some()
            || enums.iter().any(|enumeration| enumeration.name == name);
        if single {
            vec![Self::Exact(field.name.clone())]
        } else {
            vec![Self::Prefix(field.name.clone())]
        }
    }

    fn reads(&self, column: &str) -> bool {
        match self {
            Self::Exact(used) => column == used,
            Self::Prefix(used) => {
                column == used
                    || column
                        .strip_prefix(used.as_str())
                        .is_some_and(|rest| rest.starts_with('_'))
            }
        }
    }
}

struct UsedColumns {
    tables: Vec<(String, Vec<UsedColumn>)>,
}

impl UsedColumns {
    fn of(module: &Module) -> Self {
        let tables = module
            .code
            .aggregates
            .iter()
            .map(|aggregate| {
                let behaviors = aggregate
                    .all_behaviors()
                    .flat_map(behavior_columns)
                    .map(|column| UsedColumn::Exact(column.name.to_owned()));
                let fields = aggregate
                    .fields
                    .iter()
                    .flat_map(|field| UsedColumn::of(field, &module.code.enums));
                let columns = std::iter::once(UsedColumn::Exact("id".to_owned()))
                    .chain(behaviors)
                    .chain(fields)
                    .collect();
                (snake_case(&aggregate.name), columns)
            })
            .collect();
        Self { tables }
    }

    fn declares(&self, table: &str) -> bool {
        self.tables.iter().any(|(name, _)| name == table)
    }

    fn uses(&self, table: &str, column: &str) -> bool {
        self.tables
            .iter()
            .filter(|(name, _)| name == table)
            .flat_map(|(_, columns)| columns)
            .any(|used| used.reads(column))
    }
}

enum Lock {
    Rewrite { table: String, column: String },
    NotNullWithoutDefault { table: String, column: String },
    ColumnType { table: String, column: String },
    SetNotNull { table: String, column: String },
    Index { table: String },
    DropUsed { table: String, column: String },
    DropUsedTable { table: String },
    TruncateUsed { table: String },
    RenameColumn { table: String, column: String },
    RenameTable { table: String },
    UnvalidatedConstraint { table: String },
    KeyIndex { table: String },
    Maintenance { statement: &'static str },
    ConcurrentNotAlone,
}

impl Lock {
    /// The table a lock blocks; `None` for a change that breaks the running
    /// version however small the table, which no table entry may keep.
    fn locked_table(&self) -> Option<&str> {
        match self {
            Self::Rewrite { table, .. }
            | Self::ColumnType { table, .. }
            | Self::SetNotNull { table, .. }
            | Self::Index { table }
            | Self::UnvalidatedConstraint { table }
            | Self::KeyIndex { table } => Some(table),
            Self::NotNullWithoutDefault { .. }
            | Self::DropUsed { .. }
            | Self::DropUsedTable { .. }
            | Self::TruncateUsed { .. }
            | Self::RenameColumn { .. }
            | Self::RenameTable { .. }
            | Self::Maintenance { .. }
            | Self::ConcurrentNotAlone => None,
        }
    }

    fn finding(self, file: &Path, line: usize) -> Finding {
        let (message, fix) = self.explain();
        Finding {
            rule: Rule::MigrationLock,
            file: file.to_path_buf(),
            line,
            message,
            fix: format!(
                "{fix}; when the lock is acceptable (an empty table, a maintenance window), say \
                 why above the statement: `-- {ALLOW} <reason>`, or in `migrations/{ALLOW_FILE}` \
                 for a migration already applied or a table that stays small"
            ),
        }
    }

    fn explain(&self) -> (String, String) {
        match self {
            Self::Rewrite { table, column } => (
                format!(
                    "adding `{table}.{column}` fills every existing row (a volatile default, a \
                     serial, identity or stored generated column), rewriting the table under an \
                     exclusive lock"
                ),
                "expand and contract: add the column nullable or with a constant default, fill \
                 it in batches from a job, then add the default or constraint"
                    .to_owned(),
            ),
            Self::NotNullWithoutDefault { table, column } => (
                format!(
                    "`{table}.{column}` is added NOT NULL without a default, which fails on any \
                     existing row and breaks the running version's inserts"
                ),
                "expand and contract: add it with a constant default (no rewrite), or nullable, \
                 fill it, and make it NOT NULL later"
                    .to_owned(),
            ),
            Self::ColumnType { table, column } => (
                format!(
                    "changing the type of `{table}.{column}` rewrites the table under an \
                     exclusive lock, and the running version still reads the old type"
                ),
                "expand and contract: add a new column of the new type, write both, fill it in \
                 batches, move the reads over, then drop the old column"
                    .to_owned(),
            ),
            Self::SetNotNull { table, column } => (
                format!(
                    "SET NOT NULL on `{table}.{column}` scans the whole table under an exclusive \
                     lock"
                ),
                format!(
                    "add `CHECK ({column} IS NOT NULL) NOT VALID`, validate it in a later \
                     migration (`VALIDATE CONSTRAINT` takes no exclusive lock), then SET NOT \
                     NULL, which the valid check makes instant"
                ),
            ),
            Self::Index { table } => (
                format!(
                    "building an index on `{table}`, which an earlier migration created, blocks \
                     every write to it until the build ends"
                ),
                "write `CREATE INDEX CONCURRENTLY IF NOT EXISTS …` as the only statement of its \
                 migration, which sofie runs outside a transaction on Postgres"
                    .to_owned(),
            ),
            Self::DropUsed { table, column } => (
                format!(
                    "`{table}.{column}` is dropped while the schema still reads it, so every \
                     load of the aggregate fails, in this version and the running one"
                ),
                "expand and contract: remove the field from the schema and deploy, then drop the \
                 column in a later migration"
                    .to_owned(),
            ),
            Self::DropUsedTable { table } => explain_dropped_table(table),
            Self::TruncateUsed { table } => explain_truncated_table(table),
            Self::RenameColumn { table, column } => explain_rename(&format!("{table}.{column}")),
            Self::RenameTable { table } => explain_rename(table),
            Self::UnvalidatedConstraint { table } => (
                format!(
                    "adding a foreign key or check to `{table}` without NOT VALID checks every \
                     row while holding a lock that blocks writes (a foreign key on the table it \
                     references too)"
                ),
                "add it `NOT VALID`, then `VALIDATE CONSTRAINT` in a later migration, which \
                 blocks no writes"
                    .to_owned(),
            ),
            Self::KeyIndex { table } => (
                format!(
                    "adding a primary key or unique constraint to `{table}` builds its index \
                     under an exclusive lock"
                ),
                "build the index with `CREATE UNIQUE INDEX CONCURRENTLY` alone in its \
                 migration, then add the constraint `USING INDEX` it"
                    .to_owned(),
            ),
            Self::Maintenance { statement } => (
                format!(
                    "`{statement}` holds an exclusive lock on its table until it ends, blocking \
                     every request touching it"
                ),
                "for an index, write it `CONCURRENTLY` (`DROP INDEX CONCURRENTLY`, `REINDEX … \
                 CONCURRENTLY`) alone in its migration; run a table rewrite by hand in a \
                 maintenance window"
                    .to_owned(),
            ),
            Self::ConcurrentNotAlone => (
                "a concurrent index build cannot run inside a transaction, and a migration of \
                 several statements runs in one"
                    .to_owned(),
                "move the statement to a migration of its own".to_owned(),
            ),
        }
    }
}

fn explain_dropped_table(table: &str) -> (String, String) {
    (
        format!(
            "`{table}` is dropped while the schema still declares it, so every query on \
             the aggregate fails, in this version and the running one"
        ),
        "expand and contract: remove the aggregate from the schema and deploy, then drop \
         the table in a later migration"
            .to_owned(),
    )
}

fn explain_truncated_table(table: &str) -> (String, String) {
    (
        format!(
            "truncating `{table}` deletes every record the schema still declares and the \
             running version serves, under an exclusive lock"
        ),
        "delete the rows in batches from a job, or remove the aggregate from the schema \
         and deploy before emptying the table"
            .to_owned(),
    )
}

fn explain_rename(renamed: &str) -> (String, String) {
    (
        format!(
            "renaming `{renamed}` breaks the running version, which still reads and writes the \
             old name until the new one replaces it"
        ),
        "expand and contract: add the new column or table, write both from the code, fill it in \
         batches, move the reads over, then drop the old one"
            .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use sofie_model::testing::{aggregate, field, module};

    use super::*;

    fn order_columns() -> UsedColumns {
        let mut order = module("order", "modules/order");
        order.code.aggregates = vec![aggregate(
            "Order",
            vec![field("email", "Email", 2), field("total", "Money", 3)],
            "modules/order/src/order.rs",
        )];
        UsedColumns::of(&order)
    }

    fn rules(sql: &str) -> Vec<(usize, String)> {
        check_migration(
            Path::new("0002.sql"),
            &lex(sql),
            &order_columns(),
            &mut Schema::default(),
            &mut Exceptions::default(),
        )
        .into_iter()
        .map(|finding| (finding.line, finding.message))
        .collect()
    }

    fn lines(sql: &str) -> Vec<usize> {
        rules(sql).into_iter().map(|(line, _)| line).collect()
    }

    #[test]
    fn refuses_what_locks_a_table_an_earlier_migration_created() {
        let sql = "ALTER TABLE \"order\" ADD COLUMN token uuid DEFAULT gen_random_uuid();\n\
                   ALTER TABLE \"order\" ADD COLUMN number bigserial;\n\
                   ALTER TABLE \"order\" ADD COLUMN note text NOT NULL;\n\
                   ALTER TABLE \"order\" ALTER COLUMN email TYPE varchar(200);\n\
                   ALTER TABLE \"order\" ALTER email SET NOT NULL;\n\
                   CREATE INDEX order_email ON \"order\" (email);\n\
                   ALTER TABLE \"order\" DROP COLUMN total_currency;\n\
                   ALTER TABLE ONLY \"order\" ADD COLUMN placed_at timestamptz NOT NULL DEFAULT now(),\n\
                   ADD COLUMN gone text, DROP COLUMN legacy, ALTER COLUMN gone DROP DEFAULT;\n\
                   CREATE INDEX CONCURRENTLY order_total ON \"order\" (total);";

        assert_eq!(lines(sql), [1, 2, 3, 4, 5, 6, 7, 10]);
    }

    #[test]
    fn a_quote_in_a_block_comment_or_a_body_hides_no_lock() {
        let sql = "/* don't run at peak */\nCREATE INDEX order_placed ON \"order\" (placed_at);\n\
                   COMMENT ON TABLE \"order\" IS $$ isn't; $$;\n\
                   CREATE INDEX order_email ON \"order\" (email);";

        assert_eq!(lines(sql), [2, 4]);
    }

    #[test]
    fn refuses_unvalidated_constraints_key_indexes_and_table_maintenance() {
        let sql = "ALTER TABLE \"order\" ADD CONSTRAINT fk FOREIGN KEY (customer) REFERENCES c (id);\n\
                   ALTER TABLE \"order\" ADD CHECK (total > 0);\n\
                   ALTER TABLE \"order\" ADD PRIMARY KEY (id);\n\
                   ALTER TABLE \"order\" ADD CONSTRAINT one UNIQUE (email);\n\
                   DROP INDEX order_email;\n\
                   REINDEX TABLE \"order\";\n\
                   VACUUM FULL \"order\";\n\
                   CLUSTER \"order\" USING order_pkey;\n\
                   ALTER TABLE \"order\" SET TABLESPACE fast;\n\
                   ALTER TABLE \"order\" ADD CONSTRAINT fk FOREIGN KEY (c) REFERENCES c (id) NOT VALID;\n\
                   ALTER TABLE \"order\" ADD CONSTRAINT one UNIQUE USING INDEX order_email;\n\
                   ALTER TABLE \"order\" VALIDATE CONSTRAINT fk;\n\
                   VACUUM \"order\";";

        assert_eq!(lines(sql), [1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn a_table_created_in_the_same_migration_is_empty() {
        let sql = "CREATE TABLE IF NOT EXISTS audit (id uuid PRIMARY KEY);\n\
                   ALTER TABLE audit ADD COLUMN note text NOT NULL;\n\
                   CREATE UNIQUE INDEX audit_note ON audit (note);";

        assert_eq!(lines(sql), Vec::<usize>::new());
    }

    #[test]
    fn a_concurrent_build_alone_in_its_migration_passes() {
        assert_eq!(
            lines(
                "-- Orders by email.\nCREATE INDEX CONCURRENTLY IF NOT EXISTS order_email ON \"order\" (email);\n"
            ),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn an_allow_comment_with_a_reason_keeps_its_statement() {
        let sql = "-- sofie:allow migration-lock: the table is empty before the first release\n\
                   CREATE INDEX order_email ON \"order\" (email);\n\
                   CREATE INDEX order_total ON \"order\" (total);\n\
                   -- sofie:allow migration-lock:\n\
                   CREATE INDEX order_placed ON \"order\" (placed_at);";

        let findings = rules(sql);

        assert_eq!(
            findings.iter().map(|(line, _)| *line).collect::<Vec<_>>(),
            [3, 5]
        );
        assert!(findings[0].1.contains("blocks every write"), "{findings:?}");
    }

    #[test]
    fn dropping_a_column_no_field_reads_passes() {
        assert_eq!(
            lines("ALTER TABLE \"order\" DROP COLUMN IF EXISTS legacy_note, DROP CONSTRAINT x;"),
            Vec::<usize>::new()
        );
        assert_eq!(lines("ALTER TABLE \"order\" DROP COLUMN version;"), [1]);
    }

    #[test]
    fn the_allow_file_keeps_statements_of_applied_migrations_and_reports_stale_entries() {
        let mut exceptions = Exceptions::parse(
            Path::new("migration-allow.toml"),
            "[[statement]]\nfile = \"0002.sql\"\nstatement = \"create  index ORDER_EMAIL\"\n\
             reason = \"An empty table.\"\n\n[[statement]]\nfile = \"0002.sql\"\n\
             statement = \"CREATE INDEX order_gone\"\nreason = \"Fixed since.\"\n",
        )
        .unwrap();
        let sql = "CREATE INDEX order_email ON \"order\" (email);\n\
                   CREATE INDEX order_total ON \"order\" (total);";

        let findings = check_migration(
            Path::new("migrations/0002.sql"),
            &lex(sql),
            &order_columns(),
            &mut Schema::default(),
            &mut exceptions,
        );
        let stale = exceptions.stale();

        assert_eq!(
            findings
                .iter()
                .map(|finding| finding.line)
                .collect::<Vec<_>>(),
            [2]
        );
        assert_eq!(stale.len(), 1, "{stale:?}");
        assert_eq!((stale[0].rule, stale[0].line), (Rule::StaleAllowance, 6));
    }

    #[test]
    fn a_table_entry_keeps_every_lock_on_its_table_and_reports_itself_stale_unused() {
        let mut exceptions = Exceptions::parse(
            Path::new("migration-allow.toml"),
            "[[table]]\nname = \"Order\"\nreason = \"It stays small.\"\n\n\
             [[table]]\nname = \"gone\"\nreason = \"Dropped since.\"\n",
        )
        .unwrap();
        let sql = "CREATE INDEX order_email ON \"order\" (email);\n\
                   ALTER TABLE \"order\" ADD CHECK (total > 0);\n\
                   CREATE INDEX audit_action ON audit (action);";

        let findings = check_migration(
            Path::new("migrations/0002.sql"),
            &lex(sql),
            &order_columns(),
            &mut Schema::default(),
            &mut exceptions,
        );
        let stale = exceptions.stale();

        assert_eq!(
            findings
                .iter()
                .map(|finding| finding.line)
                .collect::<Vec<_>>(),
            [3]
        );
        assert_eq!(stale.len(), 1, "{stale:?}");
        assert_eq!((stale[0].rule, stale[0].line), (Rule::StaleAllowance, 5));
    }

    #[test]
    fn an_allow_file_entry_needs_its_keys_and_a_reason() {
        for (text, expected) in [
            (
                "[[statement]]\nfile = \"0002.sql\"\nstatement = \"CREATE\"\n",
                "needs `reason",
            ),
            (
                "[[statement]]\nfile = \"0002.sql\"\nreason = \"x\"\n",
                "needs `statement",
            ),
            (
                "[[statement]]\nline = \"3\"\n",
                "the keys are `file`, `statement`, `name` and `reason`",
            ),
        ] {
            let error = Exceptions::parse(Path::new("migration-allow.toml"), text)
                .err()
                .map(|error| error.message)
                .unwrap_or_default();
            assert!(error.contains(expected), "{text}: {error}");
        }
    }

    #[test]
    fn a_table_entry_keeps_locks_but_not_what_breaks_the_running_version() {
        let mut exceptions = Exceptions::parse(
            Path::new("migration-allow.toml"),
            "[[table]]\nname = \"order\"\nreason = \"It stays small.\"\n",
        )
        .unwrap();
        let sql = "ALTER TABLE \"order\" DROP COLUMN email;\n\
                   ALTER TABLE \"order\" ADD COLUMN note text NOT NULL;\n\
                   ALTER TABLE \"order\" RENAME COLUMN total TO amount;\n\
                   CREATE INDEX order_email ON \"order\" (email);";

        let findings = check_migration(
            Path::new("migrations/0002.sql"),
            &lex(sql),
            &order_columns(),
            &mut Schema::default(),
            &mut exceptions,
        );

        assert_eq!(
            findings
                .iter()
                .map(|finding| finding.line)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert!(findings[2].message.contains("renam"), "{findings:?}");
    }

    #[test]
    fn dropping_or_truncating_a_declared_table_breaks_the_running_version() {
        assert_eq!(
            lines(
                "DROP TABLE IF EXISTS legacy, \"order\" CASCADE;\n\
                 TRUNCATE TABLE ONLY audit, shop.\"order\" RESTART IDENTITY;\n\
                 TRUNCATE legacy;\n\
                 CREATE TABLE scratch (id uuid);\n\
                 DROP TABLE scratch;"
            ),
            [1, 2]
        );
    }

    #[test]
    fn an_inline_key_on_an_added_column_builds_its_index_under_the_lock() {
        let findings = rules(
            "ALTER TABLE \"order\" ADD COLUMN code text UNIQUE;\n\
             ALTER TABLE \"order\" ADD COLUMN serial_id uuid CONSTRAINT k PRIMARY KEY;\n\
             ALTER TABLE \"order\" ADD COLUMN note text REFERENCES note (id) NOT VALID;",
        );

        assert_eq!(
            findings.iter().map(|(line, _)| *line).collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(
            findings[0].1.contains("primary key or unique"),
            "{findings:?}"
        );
    }

    #[test]
    fn renaming_a_column_or_a_table_breaks_the_running_version() {
        assert_eq!(
            lines(
                "ALTER TABLE \"order\" RENAME COLUMN email TO address;\n\
                 ALTER TABLE \"order\" RENAME note TO remark;\n\
                 ALTER TABLE \"order\" RENAME CONSTRAINT a TO b;\n\
                 ALTER TABLE \"order\" ADD COLUMN \"check\" text NOT NULL;\n\
                 ALTER TABLE \"order\" RENAME TO purchase;"
            ),
            [1, 2, 4, 5]
        );
    }

    #[test]
    fn dropping_a_column_that_only_starts_like_a_field_passes() {
        assert_eq!(
            lines(
                "ALTER TABLE \"order\" DROP COLUMN email_legacy, DROP COLUMN id_old;\n\
                 ALTER TABLE \"order\" DROP COLUMN total_currency;"
            ),
            [2]
        );
    }

    #[test]
    fn widening_a_column_rewrites_nothing_but_other_type_changes_do() {
        let mut schema = Schema::default();
        schema.read(&lex(
            "CREATE TABLE \"order\" (email varchar(100), note character varying(20), \
             total numeric(10, 2), code varchar(8), placed text, size int);",
        ));
        let sql = "ALTER TABLE \"order\" ALTER COLUMN email TYPE varchar(200);\n\
                   ALTER TABLE \"order\" ALTER note TYPE text;\n\
                   ALTER TABLE \"order\" ALTER COLUMN total SET DATA TYPE numeric(12, 2);\n\
                   ALTER TABLE \"order\" ALTER COLUMN code TYPE varchar(4);\n\
                   ALTER TABLE \"order\" ALTER COLUMN placed TYPE timestamptz USING placed::timestamptz;\n\
                   ALTER TABLE \"order\" ALTER COLUMN size TYPE bigint;\n\
                   ALTER TABLE \"order\" ALTER COLUMN total TYPE numeric(14, 3);\n\
                   ALTER TABLE \"order\" ALTER COLUMN unknown TYPE text;";

        let findings = check_migration(
            Path::new("0002.sql"),
            &lex(sql),
            &order_columns(),
            &mut schema,
            &mut Exceptions::default(),
        );

        assert_eq!(
            findings
                .iter()
                .map(|finding| finding.line)
                .collect::<Vec<_>>(),
            [4, 5, 6, 7, 8]
        );
    }
}
