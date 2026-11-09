fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    english::table().expect("writing the engine's English texts");
    #[cfg(feature = "compression")]
    precompress::assets().expect("compressing the engine's assets");
}

/// `lang/en.toml` as a sorted Rust table, so an app without `i18n` writes the same English
/// without loading the translations.
mod english {
    use std::fmt::Write as _;
    use std::io;
    use std::path::PathBuf;

    const SOURCE: &str = "lang/en.toml";

    pub(crate) fn table() -> io::Result<()> {
        println!("cargo::rerun-if-changed={SOURCE}");
        let mut entries = entries(&std::fs::read_to_string(SOURCE)?)?;
        entries.sort();
        let mut table = String::from("pub(crate) const ENGLISH: &[(&str, &str)] = &[\n");
        for (key, text) in entries {
            writeln!(table, "    ({key:?}, {text:?}),").map_err(io::Error::other)?;
        }
        table.push_str("];\n");
        let out = std::env::var_os("OUT_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("cargo set no OUT_DIR"))?;
        std::fs::write(out.join("english.rs"), table)
    }

    fn entries(source: &str) -> io::Result<Vec<(String, String)>> {
        let mut table = "";
        let mut entries = Vec::new();
        for line in source.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                table = name;
                continue;
            }
            let refused =
                || io::Error::other(format!("{SOURCE}: `{line}` is not `key = \"text\"`"));
            let (name, quoted) = line.split_once(" = ").ok_or_else(refused)?;
            let text = quoted
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                .filter(|text| !text.contains(['"', '\\']))
                .ok_or_else(refused)?;
            entries.push((format!("{table}.{name}"), text.to_owned()));
        }
        Ok(entries)
    }
}

/// The best compression is affordable once per build, never per request.
#[cfg(feature = "compression")]
mod precompress {
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};

    const ASSETS: [&str; 3] = ["htmx.min.js", "engine.js", "sofie.css"];
    const BROTLI_QUALITY: u32 = 11;
    const BROTLI_WINDOW: u32 = 22;
    const BROTLI_BUFFER: usize = 4096;

    pub(crate) fn assets() -> io::Result<()> {
        let out = std::env::var_os("OUT_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("cargo set no OUT_DIR"))?;
        for name in ASSETS {
            let source = Path::new("assets").join(name);
            println!("cargo::rerun-if-changed={}", source.display());
            let bytes = std::fs::read(&source)?;
            std::fs::write(out.join(format!("{name}.br")), brotli(&bytes)?)?;
            std::fs::write(out.join(format!("{name}.gz")), gzip(&bytes)?)?;
        }
        Ok(())
    }

    fn brotli(bytes: &[u8]) -> io::Result<Vec<u8>> {
        let mut compressed = Vec::new();
        let mut writer = brotli::CompressorWriter::new(
            &mut compressed,
            BROTLI_BUFFER,
            BROTLI_QUALITY,
            BROTLI_WINDOW,
        );
        writer.write_all(bytes)?;
        writer.flush()?;
        drop(writer);
        Ok(compressed)
    }

    fn gzip(bytes: &[u8]) -> io::Result<Vec<u8>> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(bytes)?;
        encoder.finish()
    }
}
