#![cfg(feature = "image-variants")]

use std::sync::Arc;
use std::time::Duration;

use sofie::db::{Db, Dep};
use sofie::http::SigningKeys;
use sofie::postgres::Postgres;
use sofie::prelude::*;
use sofie::storage::{Bytes, ContentType, Local, Resized, Resizer, Storage, StorageKey, Variant};
use sofie::worker::Worker;
use sofie_testing::TestDatabase;

const THUMB: Variant = Variant::new("thumb", 320);

/// Writes a PNG signature: the app's own resizer would decode and scale.
struct Signature;

impl Resizer for Signature {
    fn resize(&self, _: &[u8], content_type: &ContentType, _: u32) -> Result<Resized> {
        Ok(Resized {
            bytes: Bytes::from_static(b"\x89PNG\r\n\x1a\nsmaller"),
            content_type: content_type.clone(),
        })
    }
}

#[tokio::test]
async fn a_job_takes_the_storage_and_makes_an_upload_s_variants() {
    let database = TestDatabase::create().await;
    let db = Db::new(Postgres::connect(database.url()).unwrap());
    db.schema().migrate(&[]).await.unwrap();
    let root = std::env::temp_dir().join(format!(
        "sofie-variants-{}",
        StorageKey::generate().unwrap()
    ));
    let storage =
        Storage::new(Local::new(&root, Arc::new(SigningKeys::generate().unwrap())).unwrap());
    let mut put = storage.put().await.unwrap();
    put.write(Bytes::from_static(b"\x89PNG\r\n\x1a\nlarger"))
        .await
        .unwrap();
    let original = put.finish().await.unwrap();
    sofie::db::provide(&db, Arc::new(storage.clone()));
    let key = original.clone();

    let ran = Worker::new(db)
        .every(
            "media.thumbnails",
            Duration::from_hours(1),
            move |storage: Dep<Storage>| {
                let key = key.clone();
                async move {
                    storage
                        .make_variants(&key, &[THUMB], &Signature)
                        .await
                        .map(drop)
                }
            },
        )
        .drain()
        .await
        .unwrap();

    assert_eq!(ran, 1);
    assert!(storage.exists(&original.variant(&THUMB)).await.unwrap());
    std::fs::remove_dir_all(root).unwrap();
}
