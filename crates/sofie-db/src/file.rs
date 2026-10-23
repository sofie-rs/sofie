use sofie_core::{Error, Result};

/// A file the binary carries: its source path, which development rereads, and its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct File {
    pub path: &'static str,
    pub bytes: &'static [u8],
}

impl File {
    #[must_use]
    pub const fn new(path: &'static str, bytes: &'static [u8]) -> Self {
        Self { path, bytes }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        self.path.rsplit('/').next().unwrap_or(self.path)
    }

    #[must_use]
    pub fn stem(&self) -> &'static str {
        let name = self.name();
        name.split_once('.').map_or(name, |(stem, _)| stem)
    }

    pub fn text(&self) -> Result<&'static str> {
        std::str::from_utf8(self.bytes)
            .map_err(|error| Error::internal(format!("reading {} as text", self.path), error))
    }
}

/// A file by its path from the calling crate's root, the path a development build rereads;
/// `compiled = ` embeds what the build script wrote to `OUT_DIR` from it instead, and
/// `embed!("folder", [names])` gives a `&[File]`.
#[macro_export]
macro_rules! embed {
    ($folder:literal, [$($name:literal),+ $(,)?]) => {
        &[$($crate::embed!(concat!($folder, "/", $name))),+]
    };
    ($path:expr, compiled = $compiled:expr) => {
        $crate::File {
            path: concat!(env!("CARGO_MANIFEST_DIR"), "/", $path),
            bytes: include_bytes!(concat!(env!("OUT_DIR"), "/", $compiled)),
        }
    };
    ($path:expr) => {
        $crate::File {
            path: concat!(env!("CARGO_MANIFEST_DIR"), "/", $path),
            bytes: include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $path)),
        }
    };
}

#[cfg(test)]
mod tests {
    use super::File;

    #[test]
    fn a_file_is_named_by_the_last_part_of_its_path() {
        let file = File::new("/app/modules/order/lang/de.toml", b"");

        assert_eq!(file.name(), "de.toml");
        assert_eq!(file.stem(), "de");
        assert_eq!(File::new("0001_a.sql", b"").name(), "0001_a.sql");
    }

    #[test]
    fn embedding_keeps_the_path_a_development_build_reads() {
        let file = crate::embed!("Cargo.toml");

        assert!(file.path.ends_with("/sofie-db/Cargo.toml"), "{}", file.path);
        assert_eq!(
            file.text().unwrap(),
            std::fs::read_to_string(file.path).unwrap()
        );
    }

    #[test]
    fn text_that_is_not_utf8_names_its_file() {
        let error = File::new("0001_a.sql", b"\xff").text().unwrap_err();

        assert!(
            error.to_string().starts_with("reading 0001_a.sql as text"),
            "{error}"
        );
    }
}
