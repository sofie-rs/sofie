use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use sofie_check::Rule;
use sofie_model::App;

use crate::cargo::{self, Diagnostics};
use crate::check_command::check_model;
use crate::dockerfile::{
    DOCKERIGNORE, UNPINNED, dockerfile, island_toolchain, refuse_paths_outside,
};
use crate::gen_command::{GenError, io_error, read_existing};
use crate::pgo;
use crate::source_edits::read;

/// The app's own binary, or `<app>-worker` (`worker`); `pgo` rebuilds it with
/// a profile recorded under `sofie bench`'s load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildOptions {
    pub image: bool,
    pub worker: bool,
    pub pgo: bool,
}

/// The cargo feature an app's worker binary requires, so other builds skip it.
const WORKER_FEATURE: &str = "worker-binary";
const DIST_PROFILE: &str = "dist";
const DIST_SECTION: &str =
    "[profile.dist]\ninherits = \"release\"\nlto = \"fat\"\ncodegen-units = 1\nstrip = \"symbols\"";

#[derive(Debug)]
pub enum Built {
    Binary(Binary),
    Image(Image),
}

#[derive(Debug)]
pub struct Binary {
    pub path: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Image {
    Built {
        tag: String,
    },
    Unpinned {
        dockerfile: PathBuf,
    },
    NoDocker {
        dockerfile: PathBuf,
        command: String,
    },
}

/// `--image` builds nothing on the host: the image builds the binary itself.
pub fn build(root: &Path, options: BuildOptions) -> Result<Built, GenError> {
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    if !options.image {
        return build_binary(root, &app, options).map(Built::Binary);
    }
    if options.worker || options.pgo {
        return Err(GenError::Usage(
            "`--image` builds the app's binary inside the image, without `--worker` or `--pgo`"
                .to_owned(),
        ));
    }
    refuse_findings(root, &app)?;
    let context =
        sofie_model::workspace_root(&cargo::app_manifest(root)).map_err(GenError::Model)?;
    refuse_missing_dist_profile(&context)?;
    refuse_paths_outside(root, &context)?;
    build_image(root, &app, &context).map(Built::Image)
}

pub(crate) fn build_binary(
    root: &Path,
    app: &App,
    options: BuildOptions,
) -> Result<Binary, GenError> {
    refuse_findings(root, app)?;
    let workspace = sofie_model::workspace_root(&root.join("app").join("Cargo.toml"))
        .map_err(GenError::Model)?;
    refuse_missing_dist_profile(&workspace)?;
    let package = &app.app.package;
    let worker = format!("{package}-worker");
    let mut arguments = vec!["--profile", DIST_PROFILE, "--bin"];
    if options.worker {
        refuse_missing_worker(root, &worker, app.has_feature_flags())?;
        arguments.extend([worker.as_str(), "--features", WORKER_FEATURE]);
    } else {
        arguments.push(package);
    }
    let path = if options.pgo {
        pgo::build(root, app, &arguments)?
    } else {
        cargo::build(root, &arguments, Diagnostics::Terminal)?
    };
    let bytes = fs::metadata(&path)
        .map_err(|source| io_error(&path, source))?
        .len();
    Ok(Binary { path, bytes })
}

/// Cargo refuses an undefined profile tersely; this names what to add.
fn refuse_missing_dist_profile(workspace: &Path) -> Result<(), GenError> {
    let manifest = workspace.join("Cargo.toml");
    if read(&manifest)?
        .lines()
        .any(|line| line.trim() == "[profile.dist]")
    {
        return Ok(());
    }
    Err(GenError::Usage(format!(
        "{} defines no `dist` profile, which `sofie build` builds with; add:\n\n{DIST_SECTION}",
        manifest.display()
    )))
}

/// What `sofie check` reports, so a release never embeds wiring that disagrees
/// with the code; stale files alone name `sofie gen` as the fix.
fn refuse_findings(root: &Path, app: &App) -> Result<(), GenError> {
    let findings = check_model(root, app)?;
    let stale: Option<Vec<String>> = findings
        .iter()
        .map(|finding| {
            (finding.rule == Rule::StaleWiring).then(|| finding.file.display().to_string())
        })
        .collect();
    match stale {
        _ if findings.is_empty() => Ok(()),
        Some(stale) => Err(GenError::App(format!(
            "stale generated files: {}; run `sofie gen {}`",
            stale.join(", "),
            root.display()
        ))),
        None => Err(GenError::Check(findings)),
    }
}

/// Names the two pieces cargo's own "no bin target" error would leave the reader to find.
fn refuse_missing_worker(root: &Path, worker: &str, flags: bool) -> Result<(), GenError> {
    let manifest = cargo::app_manifest(root);
    if read(&manifest)?.contains(&format!("name = \"{worker}\"")) {
        return Ok(());
    }
    // The generated `worker` takes the flags then, which `run_worker` cannot hand it.
    let run = if flags {
        "run_worker_with_flags"
    } else {
        "run_worker"
    };
    Err(GenError::Usage(format!(
        "{} declares no `{worker}` binary; add `src/bin/worker.rs` calling \
         `sofie::{run}(MODULES, worker)` and declare it:\n\
         [features]\n{WORKER_FEATURE} = []\n\n\
         [[bin]]\nname = \"{worker}\"\npath = \"src/bin/worker.rs\"\n\
         required-features = [\"{WORKER_FEATURE}\"]",
        manifest.display()
    )))
}

/// The app reaches the framework and its modules by path, so the image builds
/// from the workspace `context`.
fn build_image(root: &Path, app: &App, context: &Path) -> Result<Image, GenError> {
    let path = root.join("Dockerfile");
    let text = match read_existing(&path)? {
        Some(text) => text,
        None => write_dockerfile(app, context, &path)?,
    };
    if text.contains(UNPINNED) {
        return Ok(Image::Unpinned { dockerfile: path });
    }
    let tag = image_tag(&app.app.package);
    let built = Command::new("docker")
        .arg("build")
        .arg("--file")
        .arg(&path)
        .args(["--tag", &tag])
        .arg(context)
        .status();
    let status = match built {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let command = format!(
                "docker build --file {} --tag {tag} {}",
                path.display(),
                context.display()
            );
            return Ok(Image::NoDocker {
                dockerfile: path,
                command,
            });
        }
        built => built.map_err(|source| io_error(&path, source))?,
    };
    if !status.success() {
        return Err(GenError::App(format!(
            "`docker build` failed with {status}"
        )));
    }
    Ok(Image::Built { tag })
}

/// Docker names an image in lowercase, starting with a letter or digit; a cargo package may
/// have capitals and start with `_`.
fn image_tag(package: &str) -> String {
    package
        .trim_start_matches(|character: char| !character.is_ascii_alphanumeric())
        .to_lowercase()
}

fn write_dockerfile(app: &App, context: &Path, path: &Path) -> Result<String, GenError> {
    let toolchain = island_toolchain(app, context)?;
    let text = dockerfile(&app.app.package, toolchain.as_deref());
    fs::write(path, &text).map_err(|source| io_error(path, source))?;
    let ignore = path.with_file_name("Dockerfile.dockerignore");
    fs::write(&ignore, DOCKERIGNORE).map_err(|source| io_error(&ignore, source))?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sofie_model::testing::TemporaryDirectory;

    use super::{image_tag, refuse_missing_dist_profile, refuse_missing_worker};

    #[test]
    fn a_missing_worker_binary_is_told_to_hand_flags_when_the_app_has_them() {
        let directory = TemporaryDirectory::new("sofie-worker-binary");
        directory.write("app/Cargo.toml", "[package]\nname = \"shop\"\n");

        let plain = refuse_missing_worker(directory.root(), "shop-worker", false)
            .unwrap_err()
            .to_string();
        let flagged = refuse_missing_worker(directory.root(), "shop-worker", true)
            .unwrap_err()
            .to_string();

        assert!(
            plain.contains("`sofie::run_worker(MODULES, worker)`"),
            "{plain}"
        );
        assert!(
            flagged.contains("`sofie::run_worker_with_flags(MODULES, worker)`"),
            "{flagged}"
        );
    }

    #[test]
    fn an_image_is_tagged_with_the_package_name_lowercased() {
        assert_eq!(image_tag("My_Shop"), "my_shop");
        assert_eq!(image_tag("_Shop-2"), "shop-2");
    }

    #[test]
    fn a_workspace_without_a_dist_profile_is_told_what_to_add() {
        let workspace = std::env::temp_dir().join(format!("sofie-dist-{}", std::process::id()));
        fs::create_dir_all(&workspace).unwrap();
        let manifest = workspace.join("Cargo.toml");

        fs::write(&manifest, "[profile.release]\nlto = \"fat\"\n").unwrap();
        let missing = refuse_missing_dist_profile(&workspace)
            .unwrap_err()
            .to_string();
        fs::write(&manifest, "[profile.dist]\ninherits = \"release\"\n").unwrap();
        let present = refuse_missing_dist_profile(&workspace);
        fs::remove_dir_all(&workspace).unwrap();

        assert!(
            missing.contains("[profile.dist]\ninherits = \"release\""),
            "{missing}"
        );
        assert!(present.is_ok());
    }
}
