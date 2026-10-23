#![cfg(feature = "local")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sofie_auth::SigningKeys;
use sofie_core::{TestClock, Timestamp};
use sofie_storage::{ContentType, Local, SERVED_PATH, Served, Storage, StorageKey};

struct Root(PathBuf);

impl Drop for Root {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn root() -> Root {
    let name = format!("sofie-storage-{}", StorageKey::generate().unwrap());
    Root(std::env::temp_dir().join(name))
}

fn storage(root: &Root) -> (Storage, TestClock) {
    let keys = Arc::new(SigningKeys::generate().unwrap());
    let clock = TestClock::starting_at(Timestamp::from_second(1_700_000_000).unwrap());
    let local = Local::new(&root.0, keys).unwrap();
    (Storage::new(local).with_clock(clock.clock()), clock)
}

async fn read_all(storage: &Storage, key: &StorageKey) -> Option<Vec<u8>> {
    let mut content = storage.get(key).await.unwrap()?;
    let mut read = Vec::new();
    while let Some(chunk) = content.chunk().await.unwrap() {
        read.extend_from_slice(&chunk);
    }
    Some(read)
}

fn files_in(root: &Root) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(&root.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn a_file_written_in_chunks_appears_whole_under_its_key_once_finished() {
    let root = root();
    let (storage, _) = storage(&root);
    let file: Vec<u8> = (0..200_000_u32)
        .map(|index| u8::try_from(index % 251).unwrap())
        .collect();

    let mut put = storage.put().await.unwrap();
    for chunk in file.chunks(7_000) {
        put.write(Bytes::copy_from_slice(chunk)).await.unwrap();
    }
    let key = put.key().clone();
    assert!(!storage.exists(&key).await.unwrap());
    let partial = files_in(&root);
    assert_eq!(partial.len(), 1);
    assert!(
        partial[0].starts_with(&format!("{key}.")) && partial[0].ends_with(".partial"),
        "{partial:?}"
    );
    assert_eq!(put.size(), 200_000);
    assert_eq!(put.finish().await.unwrap(), key);

    assert_eq!(files_in(&root), [key.to_string()]);
    assert_eq!(read_all(&storage, &key).await.unwrap(), file);
    storage.delete(&key).await.unwrap();
    storage.delete(&key).await.unwrap();
    assert!(!storage.exists(&key).await.unwrap());
    assert!(read_all(&storage, &key).await.is_none());
}

#[tokio::test]
async fn an_aborted_or_dropped_write_leaves_nothing() {
    let root = root();
    let (storage, _) = storage(&root);

    let mut aborted = storage.put().await.unwrap();
    aborted.write(Bytes::from_static(b"half")).await.unwrap();
    aborted.abort().await.unwrap();
    let mut dropped = storage.put().await.unwrap();
    dropped.write(Bytes::from_static(b"half")).await.unwrap();
    drop(dropped);

    assert_eq!(files_in(&root), Vec::<String>::new());
}

#[tokio::test]
async fn a_download_link_names_its_file_until_it_expires() {
    let root = root();
    let (storage, clock) = storage(&root);
    let key = storage.put().await.unwrap().finish().await.unwrap();
    let png = ContentType::parse("image/png").unwrap();

    let link = storage
        .download_url(&key, &png, Duration::from_secs(60))
        .unwrap();

    assert!(link.starts_with(&format!(
        "{SERVED_PATH}{key}/image/png?expires=1700000060&signature="
    )));
    let served = Served {
        key: key.clone(),
        content_type: png.clone(),
        filename: None,
        fresh_for: Duration::from_secs(60),
    };
    assert_eq!(storage.served(&link), Some(served));
    let named = storage
        .named_download_url(&key, &png, "Fatura Şubat.png", Duration::from_secs(60))
        .unwrap();
    assert_eq!(
        storage
            .served(&named)
            .and_then(|served| served.filename)
            .as_deref(),
        Some("Fatura Şubat.png")
    );
    assert_eq!(storage.served(&named.replace("Fatura", "Other")), None);
    let other = StorageKey::generate().unwrap();
    for forged in [
        link.replace(key.as_str(), other.as_str()),
        link.replace("image/png", "text/html"),
        link.replace("1700000060", "1800000000"),
        format!("{SERVED_PATH}{key}/image/png"),
    ] {
        assert_eq!(storage.served(&forged), None, "{forged}");
    }
    clock.advance(Duration::from_secs(60)).unwrap();
    assert_eq!(storage.served(&link), None);
    assert!(
        storage
            .download_url(&key, &png, Duration::from_hours(7 * 24 + 1))
            .is_err()
    );
}

#[tokio::test]
async fn a_put_whose_finish_fails_leaves_no_partial_file() {
    let root = root();
    let (storage, _) = storage(&root);
    let mut put = storage.put().await.unwrap();
    let key = put.key().clone();
    std::fs::create_dir_all(root.0.join(key.as_str()).join("occupied")).unwrap();
    put.write(Bytes::from_static(b"kept nowhere"))
        .await
        .unwrap();

    assert!(put.finish().await.is_err());

    assert_eq!(files_in(&root), [key.as_str()]);
}
