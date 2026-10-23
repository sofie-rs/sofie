use crate::Write;

impl Write<'_> {
    /// Every one for an insert.
    #[must_use]
    pub fn changed_fields(&self) -> Vec<&str> {
        let columns = self.table.columns.iter().zip(self.fields);
        columns
            .filter(|(column, value)| {
                !self
                    .loaded(column)
                    .is_some_and(|loaded| crate::sql::same_value(loaded, value))
            })
            .map(|(column, _)| column.as_str())
            .collect()
    }
}
