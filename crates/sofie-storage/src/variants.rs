use bytes::Bytes;
use ring::digest::{SHA256, digest};
use sofie_core::{Error, Result};

use crate::{ContentType, Storage, StorageKey};

/// Read whole into memory for the resizer, so larger originals are refused.
pub const MAX_ORIGINAL_BYTES: u64 = 32 * 1024 * 1024;
const RESIZED: [&str; 4] = ["image/jpeg", "image/png", "image/webp", "image/gif"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Variant {
    pub name: &'static str,
    pub width: u32,
}

impl Variant {
    #[must_use]
    pub const fn new(name: &'static str, width: u32) -> Self {
        Self { name, width }
    }
}

/// The app's: the framework decodes no images, so it brings no image library.
pub trait Resizer: Send + Sync {
    fn resize(&self, image: &[u8], content_type: &ContentType, width: u32) -> Result<Resized>;
}

pub struct Resized {
    pub bytes: Bytes,
    pub content_type: ContentType,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MadeVariant {
    pub variant: Variant,
    pub key: StorageKey,
    pub content_type: ContentType,
}

impl StorageKey {
    /// Found again without being stored, and still no name a caller chose.
    #[must_use]
    pub fn variant(&self, variant: &Variant) -> Self {
        let derived = digest(
            &SHA256,
            format!("{}/{}", self.as_str(), variant.name).as_bytes(),
        );
        Self::from_hex(sofie_core::encoding::hex::encode(&derived.as_ref()[..16]))
    }
}

async fn resized(
    resizer: &'static dyn Resizer,
    image: Bytes,
    content_type: ContentType,
    variant: Variant,
) -> Result<Resized> {
    tokio::task::spawn_blocking(move || resizer.resize(&image, &content_type, variant.width))
        .await
        .map_err(|error| Error::internal("resizing an image", error))?
}

impl Storage {
    /// A variant already kept is not made again, so a retried job finishes the rest.
    /// `'static`, since resizing runs on the blocking pool rather than stall an async worker.
    pub async fn make_variants(
        &self,
        original: &StorageKey,
        variants: &[Variant],
        resizer: &'static dyn Resizer,
    ) -> Result<Vec<MadeVariant>> {
        let image = Bytes::from(self.read_image(original).await?);
        let content_type = ContentType::sniff(&image)
            .filter(|sniffed| RESIZED.contains(&sniffed.as_str()))
            .ok_or_else(|| Error::invalid("image", "is not a JPEG, PNG, WebP or GIF image"))?;
        let mut made = Vec::with_capacity(variants.len());
        for variant in variants {
            let key = original.variant(variant);
            let content_type = if let Some(kept) = self.kept_type(&key).await? {
                kept
            } else {
                let smaller =
                    resized(resizer, image.clone(), content_type.clone(), *variant).await?;
                self.put_at(&key, smaller.bytes).await?;
                smaller.content_type
            };
            made.push(MadeVariant {
                variant: *variant,
                key,
                content_type,
            });
        }
        Ok(made)
    }

    pub async fn delete_with_variants(
        &self,
        original: &StorageKey,
        variants: &[Variant],
    ) -> Result<()> {
        for variant in variants {
            self.delete(&original.variant(variant)).await?;
        }
        self.delete(original).await
    }

    async fn kept_type(&self, key: &StorageKey) -> Result<Option<ContentType>> {
        let Some(mut content) = self.get(key).await? else {
            return Ok(None);
        };
        let start = content.chunk().await?.unwrap_or_default();
        Ok(Some(
            ContentType::sniff(&start).unwrap_or_else(ContentType::octet_stream),
        ))
    }

    async fn read_image(&self, key: &StorageKey) -> Result<Vec<u8>> {
        let mut content = self.get(key).await?.ok_or(Error::NotFound)?;
        let mut image = Vec::new();
        while let Some(chunk) = content.chunk().await? {
            image.extend_from_slice(&chunk);
            if u64::try_from(image.len()).unwrap_or(u64::MAX) > MAX_ORIGINAL_BYTES {
                return Err(Error::invalid(
                    "image",
                    "is larger than 32 MiB, the most a variant is made from",
                ));
            }
        }
        Ok(image)
    }

    async fn put_at(&self, key: &StorageKey, bytes: Bytes) -> Result<()> {
        let mut writer = self.store.writer(key).await?;
        if let Err(error) = writer.write(bytes).await {
            writer.abort().await?;
            return Err(error);
        }
        writer.finish().await
    }
}
