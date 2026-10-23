use sofie_core::{SqlValue, Uuid};

use crate::Dialect;
use crate::aggregate::Table;

#[derive(Clone, Debug)]
pub struct Selected(pub(crate) Selection);

#[derive(Clone, Debug)]
pub(crate) enum Selection {
    Column(String),
    Expression(&'static str),
    #[cfg(feature = "joins")]
    Joined(crate::joins::Join),
    #[cfg(feature = "children")]
    Children(&'static crate::ChildTable),
}

impl Selected {
    pub(crate) fn column(name: impl Into<String>) -> Self {
        Self(Selection::Column(name.into()))
    }

    #[cfg(feature = "listing")]
    pub(crate) fn column_name(&self) -> Option<&str> {
        let Selection::Column(name) = &self.0 else {
            return None;
        };
        Some(name)
    }

    #[cfg(feature = "live")]
    pub(crate) fn joined_table(&self) -> Option<(&'static str, &'static str)> {
        match &self.0 {
            #[cfg(feature = "joins")]
            Selection::Joined(join) => Some(join.table()),
            _ => None,
        }
    }

    pub(crate) fn is_fixed(&self) -> bool {
        match self.0 {
            #[cfg(feature = "joins")]
            Selection::Joined(_) => false,
            _ => true,
        }
    }

    pub(crate) fn render(
        &self,
        base: &Table,
        dialect: &dyn Dialect,
        owner: Option<Uuid>,
        params: &mut Vec<SqlValue>,
    ) -> String {
        let _ = (base, owner, &params);
        match &self.0 {
            Selection::Column(name) => dialect.quote(name),
            Selection::Expression(sql) => (*sql).to_owned(),
            #[cfg(feature = "joins")]
            Selection::Joined(join) => join.render(base, dialect, owner, params),
            #[cfg(feature = "children")]
            Selection::Children(child) => child.render(base, dialect),
        }
    }
}
