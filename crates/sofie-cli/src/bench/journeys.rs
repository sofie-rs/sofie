//! The typed k6 client (`bench/gen/client.ts`) that hand-written journeys in a
//! module's `bench/*.ts` call, and the script running those journeys. A route
//! renamed or removed takes its function out of the client, so a journey
//! calling it fails as k6 loads it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sofie_model::naming::RouteSite;
use sofie_model::{App, Modifier};

use super::script::{HEADER, member, metric_name};
use crate::api::typescript::{TYPESCRIPT_HEADER, typescript};
use crate::api::{Api, Endpoint};
use crate::gen_command::GenError;
use crate::source_edits::files_with_extension;
use crate::targets::{Access, access};

const CLIENT: &str = include_str!("client.ts");
const RUNNER: &str = include_str!("journeys.ts");

pub(crate) struct Journey {
    pub(crate) name: String,
    /// From `bench/gen/`, where the script importing it lies.
    pub(crate) import: String,
}

pub(crate) struct Client {
    pub(crate) text: String,
    pub(crate) member_areas: Vec<String>,
}

pub(crate) fn client(app: &App) -> Result<Client, GenError> {
    let types = typescript(app);
    let types = types.strip_prefix(TYPESCRIPT_HEADER).unwrap_or(&types);
    let signed: Vec<String> = app
        .route_sites()
        .iter()
        .filter(|site| site.route.modifiers.contains(&Modifier::Signed))
        .map(RouteSite::name)
        .collect();
    let mut members = BTreeMap::new();
    let mut functions = String::new();
    for endpoint in Api::of(app).endpoints {
        if signed.contains(&endpoint.name) {
            continue;
        }
        let session = match access(app, &endpoint.area)? {
            Access::Anonymous => "Session<null>".to_owned(),
            Access::Member(paths) => {
                members.insert(endpoint.area.clone(), member(&paths));
                format!("Session<\"{}\">", endpoint.area)
            }
            Access::Unreachable(_) => continue,
        };
        functions.push_str(&function(&endpoint, &session));
    }
    let mut sessions = String::new();
    let mut signed_in = String::new();
    for area in members.keys() {
        let _ = writeln!(sessions, "  {area}: Session<\"{area}\">;");
        let _ = writeln!(signed_in, "    {area}: signIn(\"{area}\"),");
    }
    let member_areas = members.keys().cloned().collect();
    let members = serde_json::to_string_pretty(&json!(members)).expect("JSON serializes");
    let text = format!(
        "{HEADER}{CLIENT}\n{types}\nconst MEMBERS = {members} as const;\n\n\
         export interface Sessions {{\n{sessions}}}\n\n\
         export function signInAll(): Sessions {{\n  return {{\n{signed_in}  }};\n}}\n{functions}"
    );
    Ok(Client { text, member_areas })
}

fn function(endpoint: &Endpoint, session: &str) -> String {
    let name = &endpoint.name;
    let typed = |part: &str| format!("Endpoints[\"{name}\"][\"{part}\"]");
    let mut parameters = format!("session: {session}");
    let mut path = format!("\"{}\"", endpoint.path);
    if !endpoint.parameters.is_empty() {
        let _ = write!(parameters, ", parameters: {}", typed("parameters"));
        path = format!("fill({path}, parameters)");
    }
    let mut input = String::new();
    if endpoint.input.is_some() {
        let _ = write!(parameters, ", input: {}", typed("input"));
        input.push_str(", input");
    }
    let mut text = String::from("\n");
    if let Some(summary) = &endpoint.summary {
        let _ = writeln!(text, "/** {} */", summary.replace("*/", "* /"));
    }
    let output = typed("output");
    let _ = writeln!(
        text,
        "export function {}({parameters}): {output} {{\n  \
         return send(session, \"{name}\", \"{}\", {path}{input}) as {output};\n}}",
        name.replace('.', "_"),
        endpoint.method.name(),
    );
    text
}

/// Every module's `bench/*.ts`, named `<module>.<file stem>`.
pub(crate) fn journeys(app: &App) -> Result<Vec<Journey>, GenError> {
    let mut journeys = Vec::new();
    for module in &app.modules {
        for file in files_with_extension(&module.krate.path.join("bench"), "ts")? {
            journeys.push(Journey {
                name: format!("{}.{}", module.krate.crate_name, stem(&file)?),
                import: import_from_gen(&app.root, &file)?,
            });
        }
    }
    Ok(journeys)
}

pub(crate) fn runner(journeys: &[Journey]) -> String {
    let mut imports = String::new();
    let mut entries = Vec::new();
    for (index, journey) in journeys.iter().enumerate() {
        let _ = writeln!(imports, "import journey{index} from '{}';", journey.import);
        let name = format!("journey {}", journey.name);
        entries.push(format!(
            "  {{ name: {}, metric: {}, run: journey{index} }},",
            Value::from(name.as_str()),
            Value::from(metric_name(&name)),
        ));
    }
    format!(
        "{HEADER}{imports}\nconst JOURNEYS: Journey[] = [\n{}\n];\n\n{RUNNER}",
        entries.join("\n")
    )
}

fn stem(file: &Path) -> Result<&str, GenError> {
    file.file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| GenError::App(format!("{} has no UTF-8 name", file.display())))
}

fn import_from_gen(root: &Path, file: &Path) -> Result<String, GenError> {
    let relative: PathBuf = file
        .strip_prefix(root)
        .map_err(|_| {
            GenError::App(format!(
                "the journey {} lies outside the app at {}",
                file.display(),
                root.display()
            ))
        })?
        .to_path_buf();
    let relative = relative
        .to_str()
        .ok_or_else(|| GenError::App(format!("{} has no UTF-8 path", file.display())))?;
    Ok(format!("../../{}", relative.replace('\\', "/")))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{client, journeys, runner};

    #[test]
    fn matches_the_shops_committed_client_and_journeys() {
        let shop = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/shop");
        let app = sofie_model::load(&shop).unwrap();

        let client = client(&app).unwrap();
        let journeys = journeys(&app).unwrap();

        assert_eq!(
            client.text,
            include_str!("../../../../examples/shop/bench/gen/client.ts"),
            "{}",
            client.text
        );
        assert_eq!(client.member_areas, ["customer"]);
        assert_eq!(journeys.len(), 1);
        assert_eq!(journeys[0].name, "order.place_and_cancel");
        assert_eq!(
            journeys[0].import,
            "../../modules/order/bench/place_and_cancel.ts"
        );
        assert_eq!(
            runner(&journeys),
            include_str!("../../../../examples/shop/bench/gen/journeys.ts")
        );
    }
}
