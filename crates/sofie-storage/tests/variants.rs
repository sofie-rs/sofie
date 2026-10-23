#![cfg(all(feature = "local", feature = "variants"))]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use sofie_auth::SigningKeys;
use sofie_core::Result;
use sofie_storage::{ContentType, Local, Resized, Resizer, Storage, StorageKey, Variant};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nthe pixels";
const THUMB: Variant = Variant::new("thumb", 320);
const LARGE: Variant = Variant::new("large", 1280);

struct Root(PathBuf);

impl Drop for Root {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

/// Writes a JPEG signature and the width it was asked for, counting its calls.
struct Counting(AtomicUsize);

impl Resizer for Counting {
    fn resize(&self, _: &[u8], _: &ContentType, width: u32) -> Result<Resized> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(Resized {
            bytes: Bytes::from([b"\xff\xd8\xff".as_slice(), width.to_string().as_bytes()].concat()),
            content_type: ContentType::parse("image/jpeg")?,
        })
    }
}

fn local_storage() -> (Root, Storage) {
    let root = Root(std::env::temp_dir().join(format!(
        "sofie-variants-{}",
        StorageKey::generate().unwrap()
    )));
    let keys = Arc::new(SigningKeys::generate().unwrap());
    let storage = Storage::new(Local::new(&root.0, keys).unwrap());
    (root, storage)
}

async fn stored(storage: &Storage, bytes: &'static [u8]) -> StorageKey {
    let mut put = storage.put().await.unwrap();
    put.write(Bytes::from_static(bytes)).await.unwrap();
    put.finish().await.unwrap()
}

#[tokio::test]
async fn variants_are_kept_under_keys_derived_from_the_original_and_made_once() {
    static RESIZER: Counting = Counting(AtomicUsize::new(0));
    let (_root, storage) = local_storage();
    let original = stored(&storage, PNG).await;

    let made = storage
        .make_variants(&original, &[THUMB, LARGE], &RESIZER)
        .await
        .unwrap();
    let again = storage
        .make_variants(&original, &[THUMB, LARGE], &RESIZER)
        .await
        .unwrap();

    assert_eq!(made, again);
    assert_eq!(
        RESIZER.0.load(Ordering::Relaxed),
        2,
        "a kept variant is not made again"
    );
    assert_eq!(made[0].key, original.variant(&THUMB));
    assert_ne!(made[0].key, made[1].key);
    assert_eq!(made[0].content_type.as_str(), "image/jpeg");
    assert!(storage.exists(&original.variant(&LARGE)).await.unwrap());

    storage
        .delete_with_variants(&original, &[THUMB, LARGE])
        .await
        .unwrap();
    assert!(!storage.exists(&original).await.unwrap());
    assert!(!storage.exists(&made[0].key).await.unwrap());
}

#[tokio::test]
async fn a_file_that_is_no_image_gets_no_variants() {
    static UNUSED: Counting = Counting(AtomicUsize::new(0));
    let (_root, storage) = local_storage();
    let pdf = stored(&storage, b"%PDF-1.7 a document").await;

    let refused = storage.make_variants(&pdf, &[THUMB], &UNUSED).await;

    assert!(format!("{:?}", refused.err()).contains("is not a JPEG, PNG, WebP or GIF image"));
}

static RESIZING: AtomicBool = AtomicBool::new(false);
static ANSWERED: AtomicBool = AtomicBool::new(false);

/// Waits, blocking its thread, for a task on the same runtime to answer it.
struct Waiting;

impl Resizer for Waiting {
    fn resize(&self, _: &[u8], _: &ContentType, _: u32) -> Result<Resized> {
        RESIZING.store(true, Ordering::Release);
        let started = Instant::now();
        while !ANSWERED.load(Ordering::Acquire) {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "resizing blocked the runtime's only worker"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        Counting(AtomicUsize::new(0)).resize(&[], &ContentType::parse("image/png")?, 1)
    }
}

#[tokio::test(flavor = "current_thread")]
async fn resizing_leaves_the_async_worker_free() {
    let (_root, storage) = local_storage();
    let original = stored(&storage, PNG).await;
    let answering = tokio::spawn(async {
        while !RESIZING.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
        ANSWERED.store(true, Ordering::Release);
    });

    storage
        .make_variants(&original, &[THUMB], &Waiting)
        .await
        .unwrap();
    answering.await.unwrap();
}

/// A process killed mid-write leaves its partial file; a retried job must still finish.
#[tokio::test]
async fn a_partial_file_left_by_a_killed_process_does_not_block_its_variant() {
    static RESIZER: Counting = Counting(AtomicUsize::new(0));
    let (root, storage) = local_storage();
    let original = stored(&storage, PNG).await;
    let left = root.0.join(format!("{}.partial", original.variant(&THUMB)));
    std::fs::write(&left, b"half").unwrap();

    let made = storage.make_variants(&original, &[THUMB], &RESIZER).await;

    assert!(made.is_ok(), "{made:?}");
    assert!(storage.exists(&original.variant(&THUMB)).await.unwrap());
}
