use sofie_core::{ColumnReader, Result, SqlType, SqlValue, Uuid};

use crate::{Db, Decoding, Dialect, RowSink};

const MAX_RESERVED_ROWS: usize = 1000;

/// Records without an owner stay readable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reach {
    All,
    /// As `All`, for a principal whose changes history and audit columns name.
    AllAs(Uuid),
    OwnedBy(Uuid),
}

impl Reach {
    pub(crate) const fn owner(self) -> Option<Uuid> {
        match self {
            Self::All | Self::AllAs(_) => None,
            Self::OwnedBy(owner) => Some(owner),
        }
    }

    #[must_use]
    pub const fn actor(self) -> Option<Uuid> {
        match self {
            Self::All => None,
            Self::AllAs(actor) | Self::OwnedBy(actor) => Some(actor),
        }
    }
}

/// Has no way to write; its reach is fixed when made and cannot be widened.
#[derive(Clone)]
pub struct Read {
    pub(crate) db: Db,
    reach: Reach,
}

impl Read {
    #[must_use]
    pub fn within(db: &Db, reach: Reach) -> Self {
        Self {
            db: db.clone(),
            reach,
        }
    }

    pub(crate) fn owner(&self) -> Option<Uuid> {
        self.reach.owner()
    }

    pub(crate) fn actor(&self) -> Option<Uuid> {
        self.reach.actor()
    }

    pub(crate) fn across_owners(&self) -> Self {
        let reach = self.reach.actor().map_or(Reach::All, Reach::AllAs);
        Self::within(&self.db, reach)
    }

    pub(crate) fn dialect(&self) -> &dyn Dialect {
        self.db.driver().dialect()
    }

    /// Reserves `expected_rows` up to `MAX_RESERVED_ROWS`, as few rows may match a large limit.
    pub(crate) async fn decoded<T: Send>(
        &self,
        sql: &str,
        params: &[SqlValue],
        expected_rows: usize,
        expects: Vec<Option<SqlType>>,
        decode: impl FnMut(&mut dyn ColumnReader) -> Result<T> + Send,
    ) -> Result<Vec<T>> {
        let mut rows = Decoding::new(expects, decode);
        rows.reserve(expected_rows.min(MAX_RESERVED_ROWS));
        self.query(sql, params, &mut rows).await?;
        Ok(rows.into_decoded())
    }

    async fn query(&self, sql: &str, params: &[SqlValue], rows: &mut dyn RowSink) -> Result<()> {
        #[cfg(feature = "etag")]
        if crate::read_versions::tracking() {
            let mut fingerprinted = crate::read_versions::Fingerprinted::new(rows);
            self.db
                .driver()
                .query(sql, params, &mut fingerprinted)
                .await?;
            fingerprinted.record();
            return Ok(());
        }
        self.db.driver().query(sql, params, rows).await
    }
}
