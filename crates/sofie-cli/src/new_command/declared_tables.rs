use sofie_model::naming::{behavior_columns, snake_case, stored_columns};
use sofie_model::{Aggregate, EnumDeclaration, FieldDeclaration, Module};

/// A table as the module's aggregates declare it, besides its `id`.
pub(super) struct DeclaredTable {
    pub(super) name: String,
    pub(super) columns: Vec<DeclaredColumn>,
    pub(super) indexes: Vec<DeclaredIndex>,
    /// Fields whose columns the generator cannot tell.
    pub(super) unknown: Vec<UnknownField>,
    pub(super) unshaped: Vec<UnshapedIndex>,
}

pub(super) struct DeclaredColumn {
    pub(super) name: String,
    pub(super) sql_type: &'static str,
    pub(super) nullable: bool,
    pub(super) backfill: Option<&'static str>,
    /// An enum's stored names, which its CHECK lists.
    pub(super) allowed: Vec<String>,
    /// `#[max(120)]` and its kin as `max 120`, whose CHECK depends on the type.
    pub(super) bounds: Vec<String>,
}

#[derive(Clone)]
pub(super) struct DeclaredIndex {
    /// Named `<table>_<suffix>`, the name a duplicate's error is told by.
    pub(super) suffix: String,
    pub(super) columns: Vec<String>,
    pub(super) unique: bool,
    pub(super) only_where: Option<&'static str>,
}

impl DeclaredIndex {
    fn on(column: &str, unique: bool) -> Self {
        Self {
            suffix: column.to_owned(),
            columns: vec![column.to_owned()],
            unique,
            only_where: None,
        }
    }
}

pub(super) struct UnshapedIndex {
    pub(super) suffix: &'static str,
    pub(super) reason: String,
}

pub(super) struct UnknownField {
    pub(super) name: String,
    pub(super) type_name: String,
}

impl DeclaredTable {
    pub(super) fn may_store(&self, column: &str) -> bool {
        self.columns.iter().any(|known| known.name == column)
            || self.unknown.iter().any(|field| field.may_store(column))
    }
}

impl UnknownField {
    /// As `<field>` or `<field>_<part>`, as a value of several columns names them.
    pub(super) fn may_store(&self, column: &str) -> bool {
        column == self.name
            || column
                .strip_prefix(self.name.as_str())
                .is_some_and(|rest| rest.starts_with('_'))
    }
}

pub(super) fn declared_tables(module: &Module) -> Vec<DeclaredTable> {
    module
        .code
        .aggregates
        .iter()
        .map(|aggregate| declared_table(aggregate, &module.code.enums))
        .collect()
}

fn declared_table(aggregate: &Aggregate, enums: &[EnumDeclaration]) -> DeclaredTable {
    let mut table = DeclaredTable {
        name: snake_case(&aggregate.name),
        columns: aggregate
            .all_behaviors()
            .flat_map(behavior_columns)
            .map(|column| DeclaredColumn {
                name: column.name.to_owned(),
                sql_type: column.sql_type,
                nullable: column.nullable,
                backfill: column.backfill,
                allowed: Vec::new(),
                bounds: Vec::new(),
            })
            .collect(),
        indexes: Vec::new(),
        unknown: Vec::new(),
        unshaped: Vec::new(),
    };
    for field in &aggregate.fields {
        let Some(columns) = field_columns(field, enums) else {
            table.unknown.push(UnknownField {
                name: field.name.clone(),
                type_name: field.ty.to_string(),
            });
            continue;
        };
        let unique = field.constraints.iter().any(|name| name == "unique");
        let referenced =
            field.ty.record().is_some() || aggregate.owner.as_ref() == Some(&field.name);
        if (unique || referenced) && columns.len() == 1 {
            table.indexes.push(DeclaredIndex::on(&field.name, unique));
        }
        table.columns.extend(columns);
    }
    for behavior in aggregate.all_behaviors() {
        behavior_index(&mut table, aggregate, behavior);
    }
    table
}

/// `Slugged` refuses a duplicate by the index's name; `Default` holds one default per owner.
fn behavior_index(table: &mut DeclaredTable, aggregate: &Aggregate, behavior: &str) {
    match behavior {
        "Slugged" => table.indexes.push(DeclaredIndex::on("slug", true)),
        "UniqueCode" => table.indexes.push(DeclaredIndex::on("code", true)),
        "Default" if aggregate.owner_on_struct => table.unshaped.push(UnshapedIndex {
            suffix: "is_default",
            reason: format!(
                "`{}` is `Default` and owned on its struct, so add by hand its unique index \
                 over the owner column and `is_default`, `WHERE is_default`.",
                table.name
            ),
        }),
        "Default" => table.indexes.push(DeclaredIndex {
            suffix: "is_default".to_owned(),
            columns: aggregate
                .owner
                .iter()
                .cloned()
                .chain(["is_default".to_owned()])
                .collect(),
            unique: true,
            only_where: Some("is_default"),
        }),
        _ => {}
    }
}

fn field_columns(
    field: &FieldDeclaration,
    enums: &[EnumDeclaration],
) -> Option<Vec<DeclaredColumn>> {
    let (stored, nullable) = match (field.ty.name.as_str(), field.ty.arguments.as_slice()) {
        ("Option", [inner]) => (inner, true),
        _ => (&field.ty, false),
    };
    let bounds: Vec<String> = field
        .bounds
        .iter()
        .filter(|bound| *bound != "unique")
        .cloned()
        .collect();
    let column = |name: String, sql_type, allowed| DeclaredColumn {
        name,
        sql_type,
        nullable,
        backfill: None,
        allowed,
        bounds: bounds.clone(),
    };
    if let Some(enumeration) = enums
        .iter()
        .find(|enumeration| enumeration.name == stored.name)
    {
        return Some(vec![column(
            field.name.clone(),
            "text",
            enumeration.stored_names.clone(),
        )]);
    }
    let columns = stored_columns(stored)?
        .iter()
        .map(|part| {
            column(
                format!("{}{}", field.name, part.suffix),
                part.sql_type,
                Vec::new(),
            )
        })
        .collect();
    Some(columns)
}
