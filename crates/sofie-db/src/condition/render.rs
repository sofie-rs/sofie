use sofie_core::SqlValue;

use super::{InSelect, Kind};
use crate::Dialect;

/// Writes into the query's own text, so a condition costs no string of its own.
struct Renderer<'a> {
    dialect: &'a dyn Dialect,
    params: &'a mut Vec<SqlValue>,
    sql: &'a mut String,
}

impl Renderer<'_> {
    fn bind(&mut self, value: &SqlValue) {
        self.params.push(value.clone());
        self.dialect.push_placeholder(self.sql, self.params.len());
    }

    fn quoted(&mut self, column: &str) {
        self.dialect.push_quoted(self.sql, column);
    }

    fn render(&mut self, kind: &Kind) {
        match kind {
            Kind::Compare {
                columns,
                operator,
                values,
            } => self.each_column(columns, |renderer, index, column| {
                renderer.quoted(column);
                if values[index] == SqlValue::Null
                    && matches!(*operator, "=" | "IS NOT DISTINCT FROM")
                {
                    renderer.sql.push_str(" IS NULL");
                    return;
                }
                renderer.sql.push(' ');
                renderer.sql.push_str(operator);
                renderer.sql.push(' ');
                renderer.bind(&values[index]);
                // SQLite has no default escape; Postgres's is this one.
                if *operator == "LIKE" {
                    renderer.sql.push_str(" ESCAPE '\\'");
                }
            }),
            Kind::Between { column, low, high } => {
                self.sql.push('(');
                self.quoted(column);
                self.sql.push_str(" BETWEEN ");
                self.bind(low);
                self.sql.push_str(" AND ");
                self.bind(high);
                self.sql.push(')');
            }
            Kind::In { columns, rows } => self.render_in(columns, rows),
            Kind::Null(columns) => self.each_column(columns, |renderer, _, column| {
                renderer.quoted(column);
                renderer.sql.push_str(" IS NULL");
            }),
            #[cfg(feature = "listing")]
            Kind::Contains { column, pattern } => self.render_contains(column, pattern),
            Kind::InSelect(select) => self.render_in_select(select),
            Kind::Not(inner) => {
                self.sql.push_str("NOT ");
                self.render(inner);
            }
            Kind::Or(left, right) => {
                self.sql.push('(');
                self.render(left);
                self.sql.push_str(" OR ");
                self.render(right);
                self.sql.push(')');
            }
            #[cfg(feature = "search")]
            Kind::TextSearch { words, .. } if words.trim().is_empty() => {
                self.sql.push_str("(1 = 0)");
            }
            #[cfg(feature = "search")]
            Kind::TextSearch { column, words, .. } => {
                self.params.push(SqlValue::Text(words.clone()));
                let words = self.dialect.placeholder(self.params.len());
                let search = self
                    .dialect
                    .text_search(&self.dialect.quote(column), &words);
                self.sql.push_str(&search);
            }
        }
    }

    /// `(a … AND b …)`, one part per column.
    fn each_column(&mut self, columns: &[String], mut part: impl FnMut(&mut Self, usize, &str)) {
        self.sql.push('(');
        for (index, column) in columns.iter().enumerate() {
            if index > 0 {
                self.sql.push_str(" AND ");
            }
            part(self, index, column);
        }
        self.sql.push(')');
    }

    #[cfg(feature = "listing")]
    fn render_contains(&mut self, column: &str, pattern: &str) {
        self.params.push(SqlValue::Text(pattern.to_owned()));
        let pattern = self.dialect.placeholder(self.params.len());
        let column = self.dialect.quote(column);
        self.sql.push('(');
        self.sql.push_str(&self.dialect.lowercase(&column));
        self.sql.push_str(" LIKE ");
        self.sql.push_str(&self.dialect.lowercase(&pattern));
        self.sql.push_str(" ESCAPE '\\')");
    }

    fn render_in_select(&mut self, select: &InSelect) {
        self.sql.push('(');
        self.quoted(&select.column);
        self.render_select(select);
    }

    fn render_select(&mut self, select: &InSelect) {
        let (schema, name) = select.table;
        self.sql.push_str(" IN (SELECT ");
        self.quoted(&select.selected);
        self.sql.push_str(" FROM ");
        self.sql.push_str(&self.dialect.qualified(schema, name));
        self.sql.push_str(" WHERE ");
        self.render(&select.condition);
        for column in &select.hidden {
            self.sql.push_str(" AND (");
            self.quoted(column);
            self.sql.push_str(" IS NULL)");
        }
        self.sql.push_str("))");
    }

    fn render_in(&mut self, columns: &[String], rows: &[Vec<SqlValue>]) {
        if rows.is_empty() {
            self.sql.push_str("(1 = 0)");
            return;
        }
        if let ([column], true) = (columns, self.dialect.binds_arrays()) {
            self.sql.push('(');
            self.quoted(column);
            self.sql.push_str(" = ANY(");
            self.bind(&SqlValue::Array(rows.iter().flatten().cloned().collect()));
            self.sql.push_str("))");
            return;
        }
        self.sql.push('(');
        self.tuple(columns.len(), |renderer, index| {
            renderer.quoted(&columns[index]);
        });
        self.sql.push_str(" IN (");
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                self.sql.push_str(", ");
            }
            self.tuple(row.len(), |renderer, index| renderer.bind(&row[index]));
        }
        self.sql.push_str("))");
    }

    fn tuple(&mut self, count: usize, mut part: impl FnMut(&mut Self, usize)) {
        if count == 1 {
            part(self, 0);
            return;
        }
        self.sql.push('(');
        for index in 0..count {
            if index > 0 {
                self.sql.push_str(", ");
            }
            part(self, index);
        }
        self.sql.push(')');
    }
}

#[cfg(feature = "children")]
impl InSelect {
    /// `(value IN (SELECT …))`: whether `value` is among the rows the subquery selects.
    pub(crate) fn render_holding(
        &self,
        value: &SqlValue,
        dialect: &dyn Dialect,
        params: &mut Vec<SqlValue>,
        sql: &mut String,
    ) {
        let mut renderer = Renderer {
            dialect,
            params,
            sql,
        };
        renderer.sql.push('(');
        renderer.bind(value);
        renderer.render_select(self);
    }
}

impl Kind {
    pub(crate) fn render_into(
        &self,
        dialect: &dyn Dialect,
        params: &mut Vec<SqlValue>,
        sql: &mut String,
    ) {
        Renderer {
            dialect,
            params,
            sql,
        }
        .render(self);
    }
}
