fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    #[cfg(feature = "compression")]
    precompress::assets().expect("compressing the engine's assets");
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
