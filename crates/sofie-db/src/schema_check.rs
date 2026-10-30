use std::fmt;

use sofie_core::{Cast, Error, Result, SqlType, Uuid};

use crate::{Aggregate, Behavior, BoxFuture, Field, Migrations, Schema, Versioned};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogColumn {
    pub name: String,
    pub ty: Option<SqlType>,
    pub type_name: String,
    pub nullable: bool,
    pub filled: bool,
}

pub trait Catalog: Send + Sync {
    fn columns<'a>(
        &'a self,
        schema: &'a str,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Vec<CatalogColumn>>>;
}

#[derive(Clone, Copy)]
pub struct AggregateTable {
    module: &'static str,
    name: &'static str,
    fields: &'static [Field],
    behaviors: &'static [&'static dyn Behavior],
    #[cfg(feature = "children")]
    children: &'static [&'static crate::ChildTable],
    #[cfg(feature = "search-engine")]
    owner: fn() -> Option<&'static str>,
}

impl AggregateTable {
    #[must_use]
    pub const fn of<A: Aggregate>() -> Self {
        Self {
            module: A::MODULE,
            name: A::NAME,
            fields: A::FIELDS,
            behaviors: A::BEHAVIORS,
            #[cfg(feature = "children")]
            children: A::CHILDREN,
            #[cfg(feature = "search-engine")]
            owner: crate::aggregate::direct_owner::<A>,
        }
    }

    #[cfg(feature = "search-engine")]
    pub(crate) fn search_index(&self) -> Option<crate::SearchIndex> {
        let behaviors = self.behaviors;
        behaviors
            .iter()
            .any(|behavior| behavior.searched())
            .then(|| crate::SearchIndex {
                module: self.module,
                name: self.name,
                owner: (self.owner)(),
                hidden: behaviors.iter().find_map(|behavior| behavior.hides()),
                fields: behaviors
                    .iter()
                    .map(|behavior| behavior.search_fields())
                    .find(|fields| !fields.is_empty())
                    .unwrap_or_default(),
            })
    }

    fn tables(&self) -> Vec<ExpectedTable> {
        const ID: Field = Field::new("id", Uuid::COLUMNS, false);
        let behaviors = std::iter::once(&Versioned as &dyn Behavior)
            .chain(self.behaviors.iter().copied())
            .flat_map(Behavior::columns);
        let own = ExpectedTable::new(
            self.module,
            self.name,
            std::iter::once(&ID).chain(behaviors).chain(self.fields),
        );
        #[cfg_attr(not(feature = "children"), expect(unused_mut))]
        let mut tables = vec![own];
        #[cfg(feature = "children")]
        tables.extend(self.children.iter().map(|child| {
            let parent = Field::new(self.name, Uuid::COLUMNS, false);
            ExpectedTable::new(
                child.module,
                child.name,
                [&ID, &parent].into_iter().chain(child.fields),
            )
        }));
        tables
    }
}

impl fmt::Debug for AggregateTable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "AggregateTable({}.{})", self.module, self.name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatch {
    pub table: String,
    pub kind: MismatchKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MismatchKind {
    MissingTable,
    MissingColumn {
        column: String,
        expected: SqlType,
    },
    Type {
        column: String,
        expected: SqlType,
        found: String,
    },
    Nullable {
        column: String,
    },
    NotNull {
        column: String,
    },
    Unwritten {
        column: String,
    },
}

impl fmt::Display for Mismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.table, self.kind)
    }
}

impl fmt::Display for MismatchKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTable => write!(formatter, "the table does not exist"),
            Self::MissingColumn { column, expected } => {
                write!(formatter, "column `{column}` ({expected:?}) does not exist")
            }
            Self::Type {
                column,
                expected,
                found,
            } => write!(
                formatter,
                "column `{column}` is `{found}`, which does not store {expected:?}"
            ),
            Self::Nullable { column } => write!(
                formatter,
                "column `{column}` accepts null, which the schema never stores; add NOT NULL"
            ),
            Self::NotNull { column } => write!(
                formatter,
                "column `{column}` is NOT NULL, but the schema's field is optional"
            ),
            Self::Unwritten { column } => write!(
                formatter,
                "column `{column}` is NOT NULL without a default, and the schema never writes \
                 it, so every insert fails"
            ),
        }
    }
}

struct ExpectedTable {
    module: &'static str,
    name: &'static str,
    columns: Vec<ExpectedColumn>,
}

struct ExpectedColumn {
    name: String,
    ty: SqlType,
    nullable: bool,
}

impl ExpectedTable {
    fn new<'a>(
        module: &'static str,
        name: &'static str,
        fields: impl Iterator<Item = &'a Field>,
    ) -> Self {
        let columns = fields
            .flat_map(|field| {
                field.columns.iter().map(|column| ExpectedColumn {
                    name: crate::sql::column_name(field.name, column),
                    ty: column.ty,
                    nullable: field.nullable,
                })
            })
            .collect();
        Self {
            module,
            name,
            columns,
        }
    }

    async fn mismatches(&self, catalog: &dyn Catalog) -> Result<Vec<Mismatch>> {
        let found = catalog.columns(self.module, self.name).await?;
        let kinds = if found.is_empty() {
            vec![MismatchKind::MissingTable]
        } else {
            self.compare(&found)
        };
        let table = format!("{}.{}", self.module, self.name);
        Ok(kinds
            .into_iter()
            .map(|kind| Mismatch {
                table: table.clone(),
                kind,
            })
            .collect())
    }

    fn compare(&self, found: &[CatalogColumn]) -> Vec<MismatchKind> {
        let unwritten = found
            .iter()
            .filter(|column| !column.nullable && !column.filled)
            .filter(|column| !self.columns.iter().any(|known| known.name == column.name))
            .map(|column| MismatchKind::Unwritten {
                column: column.name.clone(),
            });
        self.columns
            .iter()
            .filter_map(|column| compare(column, found))
            .chain(unwritten)
            .collect()
    }
}

impl Schema<'_> {
    pub async fn mismatches<A: Aggregate>(self) -> Result<Vec<Mismatch>> {
        self.table_mismatches(&[AggregateTable::of::<A>()]).await
    }

    pub async fn check(self, migrations: &[Migrations]) -> Result<Vec<Mismatch>> {
        let aggregates: Vec<AggregateTable> = migrations
            .iter()
            .flat_map(|module| module.aggregates.iter().copied())
            .collect();
        self.table_mismatches(&aggregates).await
    }

    async fn table_mismatches(self, aggregates: &[AggregateTable]) -> Result<Vec<Mismatch>> {
        let catalog = self.db.driver().catalog().ok_or_else(|| {
            Error::internal_message("this database describes no tables to check a schema against")
        })?;
        let mut mismatches = Vec::new();
        for table in aggregates.iter().flat_map(AggregateTable::tables) {
            mismatches.extend(table.mismatches(catalog).await?);
        }
        Ok(mismatches)
    }
}

fn compare(expected: &ExpectedColumn, found: &[CatalogColumn]) -> Option<MismatchKind> {
    let column = expected.name.clone();
    let Some(actual) = found.iter().find(|actual| actual.name == expected.name) else {
        return Some(MismatchKind::MissingColumn {
            column,
            expected: expected.ty,
        });
    };
    if actual.ty != Some(expected.ty) {
        return Some(MismatchKind::Type {
            column,
            expected: expected.ty,
            found: actual.type_name.clone(),
        });
    }
    match (expected.nullable, actual.nullable) {
        (false, true) => Some(MismatchKind::Nullable { column }),
        (true, false) => Some(MismatchKind::NotNull { column }),
        _ => None,
    }
}
