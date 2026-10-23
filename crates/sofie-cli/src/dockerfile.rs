use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::Value;
use sofie_model::App;

use crate::cargo;
use crate::gen_command::{GenError, io_error};
use crate::islands::{TOOLCHAIN, has_islands};

pub(crate) const UNPINNED: &str = "@sha256:<digest>";

/// `.env.<environment>` holds that environment's secrets, which the builder
/// stage would keep in the build cache.
pub(crate) const DOCKERIGNORE: &str =
    "**/target\n**/node_modules\n.git\nbench/out\n**/.env*\n!**/.env.example\n";

const BUILDER: &str = "rust:1-bookworm";
const RUNTIME: &str = "gcr.io/distroless/cc-debian12:nonroot";
const PORT: u16 = 8080;

/// Written once and never rewritten, so the pinned digest and any edit stay.
/// The binary is named after the package, without the `.exe` a Windows host would add.
pub(crate) fn dockerfile(package: &str, island_toolchain: Option<&str>) -> String {
    let binary = package;
    let islands = island_toolchain.map_or_else(String::new, |toolchain| {
        format!(
            "RUN apt-get update \\\n    && apt-get install --yes --no-install-recommends nodejs npm \\\n    \
             && rm -rf /var/lib/apt/lists/*\nRUN npm ci --prefix {toolchain}\n"
        )
    });
    format!(
        "# syntax=docker/dockerfile:1\n\
         # Written by `sofie build --image`, which never rewrites it; build with\n\
         # `sofie build --image` or `docker build --file <this file> <workspace root>`.\n\
         #\n\
         # Pin both images before the first build: replace each <digest> with the\n\
         # digest `docker buildx imagetools inspect <image>` prints, so every rebuild\n\
         # uses the same toolchain and runtime. `sofie build --image` refuses until\n\
         # both are pinned.\n\
         FROM {BUILDER}{UNPINNED} AS builder\n\
         WORKDIR /build\n\
         COPY . .\n\
         {islands}\
         RUN --mount=type=cache,target=/usr/local/cargo/registry \\\n    \
         --mount=type=cache,target=/build/target \\\n    \
         cargo build --profile dist --locked --package {package} \\\n    \
         && cp target/dist/{binary} /{binary}\n\
         \n\
         # glibc, libgcc, CA certificates and tzdata (/usr/share/zoneinfo, which\n\
         # time zone values read); no shell and no package manager.\n\
         FROM {RUNTIME}{UNPINNED}\n\
         COPY --from=builder /{binary} /app/{binary}\n\
         USER nonroot:nonroot\n\
         ENV SOFIE_ADDRESS=0.0.0.0:{PORT}\n\
         EXPOSE {PORT}\n\
         # No HEALTHCHECK: without a shell or an HTTP client the image cannot run one.\n\
         # Probe GET /live for liveness and GET /ready for readiness instead. The\n\
         # binary writes nothing to disk, so it runs with `docker run --read-only`.\n\
         ENTRYPOINT [\"/app/{binary}\"]\n\
         CMD [\"serve\"]\n"
    )
}

/// `build.rs` bundles islands with the framework's Node toolchain, which the
/// builder installs only for an app that has islands.
pub(crate) fn island_toolchain(app: &App, context: &Path) -> Result<Option<String>, GenError> {
    if !has_islands(app) {
        return Ok(None);
    }
    let toolchain = canonical(Path::new(TOOLCHAIN))?;
    let relative = toolchain.strip_prefix(canonical(context)?).map_err(|_| {
        GenError::Usage(format!(
            "the island toolchain at {} is outside the image's build context {}",
            toolchain.display(),
            context.display()
        ))
    })?;
    Ok(Some(relative.display().to_string()))
}

/// `sofie new app` reaches the framework by an absolute path; outside the
/// build context, `cargo build` inside the image cannot find it.
pub(crate) fn refuse_paths_outside(root: &Path, context: &Path) -> Result<(), GenError> {
    let manifest = cargo::app_manifest(root);
    let output = cargo::command()
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(&manifest)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| io_error(&manifest, source))?;
    if !output.status.success() {
        return Err(GenError::App(format!(
            "`cargo metadata` for {} failed: {}",
            manifest.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let metadata: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| GenError::App(format!("`cargo metadata` answered no JSON: {error}")))?;
    let outside = outside_context(&metadata, &canonical(context)?);
    if outside.is_empty() {
        return Ok(());
    }
    Err(GenError::Usage(format!(
        "the image builds from {}, but these path dependencies lie outside it, so docker cannot \
         copy them in: {}; move them into the workspace or build the image from a directory \
         holding both",
        context.display(),
        outside.join(", ")
    )))
}

fn outside_context(metadata: &Value, context: &Path) -> Vec<String> {
    let packages = metadata["packages"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    packages
        .iter()
        .filter(|package| package["source"].is_null())
        .filter_map(|package| {
            let manifest = Path::new(package["manifest_path"].as_str()?);
            let manifest = manifest
                .canonicalize()
                .unwrap_or_else(|_| manifest.to_path_buf());
            (!manifest.starts_with(context)).then(|| {
                let directory = manifest.parent().unwrap_or(&manifest);
                format!(
                    "{} ({})",
                    package["name"].as_str().unwrap_or("?"),
                    directory.display()
                )
            })
        })
        .collect()
}

fn canonical(path: &Path) -> Result<PathBuf, GenError> {
    path.canonicalize().map_err(|source| io_error(path, source))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::{UNPINNED, dockerfile};

    #[test]
    fn builds_in_a_pinned_rust_image_and_runs_as_nonroot_on_a_pinned_distroless() {
        let text = dockerfile("shop", None);

        assert!(text.contains(&format!("FROM rust:1-bookworm{UNPINNED} AS builder")));
        assert!(text.contains("cargo build --profile dist --locked --package shop"));
        assert!(text.contains(&format!(
            "FROM gcr.io/distroless/cc-debian12:nonroot{UNPINNED}\n"
        )));
        assert!(text.contains("COPY --from=builder /shop /app/shop\nUSER nonroot:nonroot\n"));
        assert!(text.contains("ENTRYPOINT [\"/app/shop\"]\nCMD [\"serve\"]\n"));
        assert!(text.contains("ENV SOFIE_ADDRESS=0.0.0.0:8080\n"));
        assert!(text.contains("/live") && text.contains("/ready"));
        assert!(!text.contains("\nHEALTHCHECK"));
        assert!(!text.contains("nodejs"));
    }

    #[test]
    fn names_each_path_dependency_outside_the_build_context() {
        let metadata = json!({ "packages": [
            { "name": "shop", "source": null, "manifest_path": "/work/shop/app/Cargo.toml" },
            { "name": "sofie-framework", "source": null,
              "manifest_path": "/home/me/sofie/crates/sofie/Cargo.toml" },
            { "name": "serde", "source": "registry+https://github.com/rust-lang/crates.io-index",
              "manifest_path": "/home/me/.cargo/registry/serde/Cargo.toml" },
        ]});

        let outside = super::outside_context(&metadata, Path::new("/work/shop"));

        assert_eq!(outside, ["sofie-framework (/home/me/sofie/crates/sofie)"]);
    }

    #[test]
    fn keeps_every_env_file_but_the_example_out_of_the_build_context() {
        assert!(super::DOCKERIGNORE.contains("\n**/.env*\n!**/.env.example\n"));
    }

    #[test]
    fn installs_the_island_toolchain_only_for_an_app_with_islands() {
        let text = dockerfile("shop", Some("tools/web"));

        assert!(text.contains("apt-get install --yes --no-install-recommends nodejs npm"));
        assert!(text.contains("RUN npm ci --prefix tools/web\n"));
    }
}
