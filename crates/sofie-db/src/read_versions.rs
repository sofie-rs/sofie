use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};

use sofie_core::{ColumnReader, Date, Result, SqlType, SqlValue, Timestamp, Uuid};

use crate::RowSink;

// Task-local rather than carried by `Read`, so the rows a lookup reads
// through its own `Db` count too.
tokio::task_local! {
    static READS: RefCell<Reads>;
}

struct Reads {
    hasher: DefaultHasher,
    untracked: bool,
}

/// `seed` keys the fingerprint by the answer's other inputs; `None` when one has no version.
pub async fn track_reads<F: Future>(seed: u64, future: F) -> (F::Output, Option<u64>) {
    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    let reads = Reads {
        hasher,
        untracked: false,
    };
    READS
        .scope(RefCell::new(reads), async {
            let output = future.await;
            (output, reads_so_far())
        })
        .await
}

/// Every row read so far in this task; `None` outside `track_reads` or after `read_untracked`.
#[must_use]
pub fn reads_so_far() -> Option<u64> {
    READS
        .try_with(|reads| {
            let reads = reads.borrow();
            (!reads.untracked).then(|| reads.hasher.clone().finish())
        })
        .ok()
        .flatten()
}

/// The answer depends on another app's or a vendor's reply, which no fingerprint here follows.
pub fn read_untracked() {
    let _ = READS.try_with(|reads| reads.borrow_mut().untracked = true);
}

/// The answer depends on a cached value, which changes the fingerprint as a read row does.
pub fn read_cached(value: &str) {
    let _ = READS.try_with(|reads| value.hash(&mut reads.borrow_mut().hasher));
}

pub(crate) fn tracking() -> bool {
    READS.try_with(|_| ()).is_ok()
}

/// Hashes every value its rows' targets read, so a changed row changes the fingerprint.
pub(crate) struct Fingerprinted<'s> {
    rows: &'s mut dyn RowSink,
    hasher: DefaultHasher,
    count: u64,
}

impl<'s> Fingerprinted<'s> {
    pub(crate) fn new(rows: &'s mut dyn RowSink) -> Self {
        Self {
            rows,
            hasher: DefaultHasher::new(),
            count: 0,
        }
    }

    pub(crate) fn record(self) {
        let _ = READS.try_with(|reads| {
            let hasher = &mut reads.borrow_mut().hasher;
            self.count.hash(hasher);
            self.hasher.finish().hash(hasher);
        });
    }
}

impl RowSink for Fingerprinted<'_> {
    fn expects(&self) -> &[Option<SqlType>] {
        self.rows.expects()
    }

    fn row(&mut self, row: &mut dyn ColumnReader) -> Result<()> {
        self.count += 1;
        self.rows.row(&mut Hashing {
            row,
            hasher: &mut self.hasher,
        })
    }
}

struct Hashing<'r> {
    row: &'r mut dyn ColumnReader,
    hasher: &'r mut DefaultHasher,
}

macro_rules! hashed {
    ($($method:ident -> $type:ty),*) => {
        $(fn $method(&mut self) -> Result<$type> {
            let value = self.row.$method()?;
            value.hash(self.hasher);
            Ok(value)
        })*
    };
}

impl ColumnReader for Hashing<'_> {
    fn remaining(&self) -> usize {
        self.row.remaining()
    }

    fn is_null_at(&self, ahead: usize) -> Result<bool> {
        self.row.is_null_at(ahead)
    }

    fn skip(&mut self) -> Result<()> {
        self.value().map(drop)
    }

    hashed!(bool -> bool, int -> i64, uuid -> Uuid, timestamp -> Timestamp, date -> Date,
        date_time -> sofie_core::DateTime, text -> &str, bytes -> &[u8]);

    fn float(&mut self) -> Result<f64> {
        let value = self.row.float()?;
        value.to_bits().hash(self.hasher);
        Ok(value)
    }

    fn json(&mut self) -> Result<serde_json::Value> {
        let value = self.row.json()?;
        value.to_string().hash(self.hasher);
        Ok(value)
    }

    fn value(&mut self) -> Result<SqlValue> {
        let value = self.row.value()?;
        hash_value(&value, self.hasher);
        Ok(value)
    }
}

fn hash_value(value: &SqlValue, hasher: &mut DefaultHasher) {
    std::mem::discriminant(value).hash(hasher);
    match value {
        SqlValue::Null => {}
        SqlValue::Bool(value) => value.hash(hasher),
        SqlValue::Int(value) => value.hash(hasher),
        SqlValue::Float(value) => value.to_bits().hash(hasher),
        SqlValue::Text(value) => value.hash(hasher),
        SqlValue::Bytes(value) => value.hash(hasher),
        SqlValue::Uuid(value) => value.hash(hasher),
        SqlValue::Timestamp(value) => value.hash(hasher),
        SqlValue::Date(value) => value.hash(hasher),
        SqlValue::DateTime(value) => value.hash(hasher),
        SqlValue::Json(value) => value.to_string().hash(hasher),
        SqlValue::Array(values) => {
            values.len().hash(hasher);
            for value in values {
                hash_value(value, hasher);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sofie_core::RowReader;

    use super::*;
    use crate::Decoding;

    fn row(version: i64) -> Vec<SqlValue> {
        vec![SqlValue::Int(version), SqlValue::Text("open".to_owned())]
    }

    fn read(rows: &[Vec<SqlValue>], skip_version: bool) {
        let mut decoding = Decoding::new(Vec::new(), |row: &mut dyn ColumnReader| {
            if skip_version {
                row.skip()?;
            } else {
                row.int()?;
            }
            row.text().map(str::to_owned)
        });
        let mut fingerprinted = Fingerprinted::new(&mut decoding);
        for values in rows {
            fingerprinted.row(&mut RowReader::new(values)).unwrap();
        }
        fingerprinted.record();
    }

    fn fingerprint(seed: u64, rows: &[Vec<SqlValue>], skip_version: bool) -> u64 {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let reading = async { read(rows, skip_version) };
        runtime.block_on(track_reads(seed, reading)).1.unwrap()
    }

    fn tracked(reading: impl FnOnce()) -> Option<u64> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(track_reads(1, async { reading() })).1
    }

    #[test]
    fn the_fingerprint_follows_the_values_read_and_the_seed() {
        let first = fingerprint(1, &[row(1)], false);

        assert_eq!(first, fingerprint(1, &[row(1)], false));
        assert_ne!(first, fingerprint(1, &[row(2)], false));
        assert_ne!(first, fingerprint(2, &[row(1)], false));
        assert_ne!(first, fingerprint(1, &[row(1), row(1)], false));
    }

    #[test]
    fn a_skipped_column_still_counts() {
        assert_ne!(
            fingerprint(1, &[row(1)], true),
            fingerprint(1, &[row(2)], true)
        );
    }

    #[test]
    fn an_answer_from_outside_is_not_fingerprinted() {
        assert_eq!(tracked(read_untracked), None);
    }

    #[test]
    fn a_cached_value_read_changes_the_fingerprint() {
        let first = tracked(|| read_cached("{\"total\":1}"));

        assert!(first.is_some());
        assert_eq!(first, tracked(|| read_cached("{\"total\":1}")));
        assert_ne!(first, tracked(|| read_cached("{\"total\":2}")));
    }

    #[test]
    fn nothing_is_tracked_outside_a_scope() {
        read(&[row(1)], false);

        assert_eq!(reads_so_far(), None);
    }
}
