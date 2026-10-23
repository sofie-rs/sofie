use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use sofie_model::{App, island_bundles};

use crate::cargo::Diagnostics;
use crate::clients::{absolute, check_toolchain, run_node};
use crate::gen_command::{GenError, io_error};

/// Shipped inside this crate, so an installed CLI finds it beside its own sources.
pub(crate) const TOOLCHAIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/web");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IslandMode {
    Release,
    Development,
}

impl IslandMode {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::Development => "development",
        }
    }
}

#[must_use]
pub fn has_islands(app: &App) -> bool {
    app.modules.iter().any(|module| !module.islands.is_empty())
}

pub(crate) fn build_islands(
    app: &App,
    out: &Path,
    mode: IslandMode,
    diagnostics: Diagnostics,
) -> Result<Vec<String>, GenError> {
    let bundles = island_bundles(&app.modules);
    if bundles.is_empty() {
        return Ok(Vec::new());
    }
    check_toolchain()?;
    fs::create_dir_all(out).map_err(|source| io_error(out, source))?;
    let plan = out.join("plan.json");
    let text = plan_json(out, mode, &bundles);
    fs::write(&plan, text).map_err(|source| io_error(&plan, source))?;
    run_node("build.mjs", &plan, "the island build", diagnostics)?;
    Ok(bundles.into_iter().map(|bundle| bundle.area).collect())
}

#[cfg(feature = "typescript")]
pub(crate) fn types_file(app: &App) -> (PathBuf, Option<String>) {
    let text = has_islands(app).then(|| crate::typescript(app));
    (app.root.join("web").join("gen").join("types.ts"), text)
}

#[must_use]
pub fn island_sources(app: &App) -> Vec<PathBuf> {
    let toolchain = Path::new(TOOLCHAIN);
    let mut sources: Vec<PathBuf> = ["build.mjs", "shared.mjs", "runtime.js", "package-lock.json"]
        .into_iter()
        .map(|file| toolchain.join(file))
        .collect();
    sources.extend(
        app.modules
            .iter()
            .flat_map(|module| &module.islands)
            .map(|island| island.file.clone()),
    );
    sources
}

fn plan_json(out: &Path, mode: IslandMode, bundles: &[sofie_model::IslandBundle<'_>]) -> String {
    let bundles: Vec<serde_json::Value> = bundles
        .iter()
        .map(|bundle| {
            let islands: Vec<serde_json::Value> = bundle
                .islands
                .iter()
                .map(|island| json!({ "tag": island.name, "file": absolute(&island.file) }))
                .collect();
            json!({ "area": bundle.area, "islands": islands })
        })
        .collect();
    json!({ "mode": mode.name(), "out": out.display().to_string(), "bundles": bundles }).to_string()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sofie_model::Island;
    use sofie_model::testing::{TemporaryDirectory, app, area, module};

    use super::*;

    fn island(name: &str, area: &str) -> Island {
        Island {
            name: name.to_owned(),
            file: PathBuf::from(format!(
                "/shop/modules/order/src/areas/{area}/islands/{name}.svelte"
            )),
            area: Some(area.to_owned()),
            props: None,
        }
    }

    #[test]
    fn the_plan_names_each_areas_islands() {
        let mut order = module("order", "/shop/modules/order");
        order.islands = vec![island("order-chart", "member")];
        let shop = app("/shop", vec![order]);
        let bundles = island_bundles(&shop.modules);

        let plan = plan_json(Path::new("/out \"x\""), IslandMode::Release, &bundles);

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&plan).unwrap(),
            json!({
                "mode": "release",
                "out": "/out \"x\"",
                "bundles": [{
                    "area": "member",
                    "islands": [{
                        "tag": "order-chart",
                        "file": "/shop/modules/order/src/areas/member/islands/order-chart.svelte",
                    }],
                }],
            })
        );
        assert!(has_islands(&shop));
    }

    #[test]
    fn an_app_without_islands_builds_nothing() {
        let shop = app("/shop", vec![module("order", "/shop/modules/order")]);

        let built = build_islands(
            &shop,
            Path::new("/nonexistent"),
            IslandMode::Release,
            Diagnostics::Terminal,
        )
        .unwrap();

        assert_eq!(built, Vec::<String>::new());
        assert!(!has_islands(&shop));
    }

    const ISLAND: &str = r#"<script lang="ts">
  let { count }: { count: number } = $props();
</script>

<p class="chart">{count} orders</p>

<style>
  .chart { color: rebeccapurple; }
</style>
"#;

    #[test]
    fn each_area_gets_one_bundle_with_its_styles_apart() {
        let directory = TemporaryDirectory::new("sofie-islands");
        let file = directory.write("order/src/areas/member/islands/order-chart.svelte", ISLAND);
        let mut order = module("order", directory.path("order"));
        order.islands = vec![Island {
            name: "order-chart".to_owned(),
            file,
            area: Some("member".to_owned()),
            props: None,
        }];
        let shop = app(directory.root(), vec![order]);
        let out = directory.path("out");

        let built = build_islands(&shop, &out, IslandMode::Release, Diagnostics::Terminal).unwrap();

        assert_eq!(built, ["member"]);
        let script = fs::read_to_string(out.join("member.js")).unwrap();
        assert!(script.contains("\"order-chart\""), "{script}");
        assert!(!script.contains("<style"), "{script}");
        let stylesheet = fs::read_to_string(out.join("member.css")).unwrap();
        assert!(
            stylesheet.contains(".chart.svelte-") && stylesheet.contains("color"),
            "{stylesheet}"
        );
        for file in ["member.js", "member.css"] {
            let gzip = fs::read(out.join(format!("{file}.gz"))).unwrap();
            assert_eq!(gzip[..2], [0x1f, 0x8b], "{file}.gz is gzip");
            assert!(fs::metadata(out.join(format!("{file}.br"))).unwrap().len() > 0);
        }
    }

    #[test]
    fn a_broken_island_fails_the_build_naming_its_file() {
        let directory = TemporaryDirectory::new("sofie-islands");
        let file = directory.write("order/web/broken-chart.svelte", "<p>{</p>");
        let mut order = module("order", directory.path("order"));
        order.islands = vec![Island {
            name: "broken-chart".to_owned(),
            file,
            area: None,
            props: None,
        }];
        order.areas.push(area(&order, "web", Vec::new()));
        let shop = app(directory.root(), vec![order]);

        let error = build_islands(
            &shop,
            &directory.path("out"),
            IslandMode::Development,
            Diagnostics::Terminal,
        )
        .unwrap_err()
        .to_string();

        assert!(
            error.starts_with("the island build failed") && error.contains("broken-chart.svelte"),
            "{error}"
        );
    }

    #[cfg(all(unix, feature = "mcp"))]
    #[test]
    fn node_runs_in_a_process_group_of_its_own_under_sofie_mcp() {
        let directory = TemporaryDirectory::new("sofie-node");
        let script = directory.write(
            "group.mjs",
            "import { execSync } from 'node:child_process';\n\
             import { writeFileSync } from 'node:fs';\n\
             const group = execSync(`ps -o pgid= -p ${process.pid}`).toString().trim();\n\
             writeFileSync(process.argv[2], `${group} ${process.pid}`);\n",
        );
        let written = directory.path("group.txt");

        run_node(
            &script.display().to_string(),
            &written,
            "the probe",
            Diagnostics::Captured,
        )
        .unwrap();

        let text = fs::read_to_string(&written).unwrap();
        let (group, pid) = text.split_once(' ').unwrap();
        assert_eq!(group, pid);
    }
}
