use sofie_core::Fnv1a64;

use crate::render::push_display;

#[cfg(feature = "compression")]
mod precompressed;
#[cfg(feature = "compression")]
pub use precompressed::Precompressed;
#[cfg(feature = "islands")]
mod island_bundle;

/// Served under its content hash, so a browser may cache it until the file changes.
#[derive(Clone, Copy, Debug)]
pub struct Asset {
    stem: &'static str,
    extension: &'static str,
    pub bytes: &'static [u8],
    pub content_type: &'static str,
    hash: u64,
    #[cfg(feature = "compression")]
    precompressed: Option<Precompressed>,
}

/// Its compressed copies are the files beside it with `.br` and `.gz` added.
#[cfg(feature = "compression")]
#[doc(hidden)]
#[macro_export]
macro_rules! compressed_asset {
    ($asset:expr, $file:expr) => {
        $asset.with_precompressed($crate::Precompressed {
            brotli: include_bytes!(concat!($file, ".br")),
            gzip: include_bytes!(concat!($file, ".gz")),
        })
    };
}

#[cfg(not(feature = "compression"))]
#[doc(hidden)]
#[macro_export]
macro_rules! compressed_asset {
    ($asset:expr, $file:expr) => {
        $asset
    };
}

/// Compressed by the engine's build script.
macro_rules! engine_asset {
    ($stem:literal, $extension:literal, $content_type:expr) => {
        $crate::compressed_asset!(
            Asset::new(
                $stem,
                $extension,
                include_bytes!(concat!("../assets/", $stem, ".", $extension)),
                $content_type,
            ),
            concat!(env!("OUT_DIR"), "/", $stem, ".", $extension)
        )
    };
}

/// htmx, pinned; see `assets/PROVENANCE`.
pub const CLIENT_SCRIPT: Asset = engine_asset!("htmx", "min.js", JAVASCRIPT);

/// Field state htmx cannot keep, menus and panels, and a notice for an unanswered request.
pub const ENGINE_SCRIPT: Asset = engine_asset!("engine", "js", JAVASCRIPT);

pub const STYLESHEET: Asset = engine_asset!("sofie", "css", CSS);

pub(crate) const JAVASCRIPT: &str = "application/javascript; charset=utf-8";
pub(crate) const CSS: &str = "text/css; charset=utf-8";

impl Asset {
    pub(crate) const fn new(
        stem: &'static str,
        extension: &'static str,
        bytes: &'static [u8],
        content_type: &'static str,
    ) -> Self {
        Self {
            stem,
            extension,
            bytes,
            content_type,
            hash: Fnv1a64::new().write(bytes).finish(),
            #[cfg(feature = "compression")]
            precompressed: None,
        }
    }

    #[must_use]
    pub fn path(&self) -> String {
        let mut path = String::new();
        self.push_path(&mut path);
        path
    }

    pub(crate) fn push_path(&self, out: &mut String) {
        let (stem, hash, extension) = (self.stem, self.hash, self.extension);
        push_display(out, format_args!("/_sofie/{stem}-{hash:016x}.{extension}"));
    }
}

#[cfg(test)]
mod tests {
    use super::{CLIENT_SCRIPT, STYLESHEET};

    const PROVENANCE: &str = include_str!("../assets/PROVENANCE");

    #[test]
    fn the_client_script_is_the_file_recorded_in_provenance() {
        let recorded = PROVENANCE
            .lines()
            .find_map(|line| line.strip_prefix("fnv1a64 "))
            .expect("PROVENANCE records the fnv1a64 of htmx.min.js");

        assert_eq!(
            CLIENT_SCRIPT.path(),
            format!("/_sofie/htmx-{recorded}.min.js")
        );
    }

    #[test]
    fn paths_name_the_content() {
        let path = STYLESHEET.path();
        assert_eq!(path.len(), "/_sofie/sofie-".len() + 16 + ".css".len());
        assert!(
            path.starts_with("/_sofie/sofie-") && path.contains(".css"),
            "{path}"
        );
        assert_eq!(STYLESHEET.content_type, "text/css; charset=utf-8");
    }
}
