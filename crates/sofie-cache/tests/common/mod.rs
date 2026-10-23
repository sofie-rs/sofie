use sofie_core::{Cast, ColumnReader, Result, SqlValue};
use sofie_db::{Aggregate, Context, Db, Field, Meta, Reach};

pub struct Book {
    pub meta: Meta,
    pub title: String,
}

impl Aggregate for Book {
    const NAME: &'static str = "books";
    const MODULE: &'static str = "shelf";
    const FIELDS: &'static [Field] = &[Field::new("title", String::COLUMNS, false)];

    fn meta(&self) -> &Meta {
        &self.meta
    }

    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }

    fn encode(&self, out: &mut Vec<SqlValue>) {
        self.title.encode(out);
    }

    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self> {
        Ok(Self {
            meta,
            title: String::decode(row)?,
        })
    }
}

pub struct TestContext(pub Db);

impl Context for TestContext {
    fn db(&self) -> &Db {
        &self.0
    }

    fn reach(&self) -> Reach {
        Reach::All
    }
}
