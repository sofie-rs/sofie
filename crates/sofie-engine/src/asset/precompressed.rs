use super::Asset;

#[derive(Clone, Copy, Debug)]
pub struct Precompressed {
    pub brotli: &'static [u8],
    pub gzip: &'static [u8],
}

impl Asset {
    #[must_use]
    pub const fn with_precompressed(mut self, precompressed: Precompressed) -> Self {
        self.precompressed = Some(precompressed);
        self
    }

    #[must_use]
    pub const fn precompressed(&self) -> Option<Precompressed> {
        self.precompressed
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use crate::{CLIENT_SCRIPT, ENGINE_SCRIPT, STYLESHEET};

    #[test]
    fn each_engine_asset_decompresses_to_its_own_bytes() {
        for asset in [CLIENT_SCRIPT, ENGINE_SCRIPT, STYLESHEET] {
            let precompressed = asset.precompressed().unwrap();
            let mut from_brotli = Vec::new();
            brotli::Decompressor::new(precompressed.brotli, 4096)
                .read_to_end(&mut from_brotli)
                .unwrap();
            let mut from_gzip = Vec::new();
            flate2::read::GzDecoder::new(precompressed.gzip)
                .read_to_end(&mut from_gzip)
                .unwrap();

            assert_eq!(from_brotli, asset.bytes);
            assert_eq!(from_gzip, asset.bytes);
            assert!(precompressed.brotli.len() < asset.bytes.len() / 2);
        }
    }
}
