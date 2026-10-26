//! Dry by design: the plan is printed, and only `--write` carries it out.

use std::fmt::{self, Write as _};
use std::fs;
use std::path::{Component, Path, PathBuf};

use sofie_model::naming::{contract_package, relative_path};
use sofie_model::{App, Module};

use crate::env_example::remote_prefix;
use crate::gen_command::{GenError, GenMode, GenOutcome, io_error, read_existing, regenerated};
use crate::new_command::{
    app_sources, checked_name, dependency_line_start, find_module, refuse_existing, write_all,
};
use crate::source_edits::{
    DEPENDENCIES, Journal, path_dependency, read, toml_escaped, toml_section, toml_table,
    with_dependency_line,
};
use crate::wiring::{crossing_traits, exposed_aggregates};

const FEATURE: &str = "remote-contracts";
const PROXY_TABLE: &str = "proxy.toml";

#[derive(Debug)]
pub struct SplitPlan {
    pub module: String,
    pub package: String,
    /// The app the module leaves.
    pub app: String,
    pub new_app: String,
    pub root: PathBuf,
    pub from: PathBuf,
    pub to: PathBuf,
    pub workspace: PathBuf,
    pub contracts: Vec<String>,
    pub stays_local: Vec<String>,
    pub lookups: Vec<String>,
    pub events: Vec<String>,
    /// Modules of the app it leaves whose contracts the module calls, served back to it.
    pub calls_back: Vec<String>,
    /// Modules it calls that an earlier split already moved elsewhere.
    pub reaches: Vec<String>,
    pub proxy: Vec<String>,
    pub shared_prefixes: Vec<String>,
    /// The app's test files naming the module's crate, which it no longer depends on.
    pub orphaned_tests: Vec<PathBuf>,
    pub last_module: bool,
}

#[derive(Debug)]
pub struct Split {
    pub plan: SplitPlan,
    pub generated: Vec<GenOutcome>,
}

pub fn split_plan(root: &Path, module: &str) -> Result<SplitPlan, GenError> {
    let root = fs::canonicalize(root).map_err(|error| io_error(root, error))?;
    let app = sofie_model::load(&root).map_err(GenError::Model)?;
    let module = find_module(&app, checked_name(module)?)?;
    let workspace =
        sofie_model::workspace_root(&app.app.path.join("Cargo.toml")).map_err(GenError::Model)?;
    let orphaned_tests = tests_naming(&app.app.path.join("tests"), &module.krate.crate_name)?;
    Ok(plan(&app, module, &root, workspace, orphaned_tests))
}

/// Only `<crate>::` paths count: the crate's name alone is often a word, `order` a variable.
fn tests_naming(tests: &Path, crate_name: &str) -> Result<Vec<PathBuf>, GenError> {
    let mut naming = Vec::new();
    for file in sofie_model::rust_files(tests).map_err(GenError::Model)? {
        if names_crate(&read(&file)?, crate_name) {
            naming.push(file);
        }
    }
    Ok(naming)
}

fn names_crate(source: &str, crate_name: &str) -> bool {
    let path = format!("{crate_name}::");
    source.match_indices(&path).any(|(start, _)| {
        !source[..start]
            .chars()
            .next_back()
            .is_some_and(|before| before.is_alphanumeric() || before == '_' || before == ':')
    })
}

pub fn split_module(root: &Path, module: &str) -> Result<Split, GenError> {
    let plan = split_plan(root, module)?;
    refuse_existing(&plan.to)?;
    let mut journal = Journal::default();
    let generated = carry_out(&mut journal, &plan);
    let generated = journal.finish(generated)?;
    Ok(Split { plan, generated })
}

/// Every step through the journal, so a failure, `sofie gen`'s findings on
/// either app among them, undoes the split.
fn carry_out(journal: &mut Journal, plan: &SplitPlan) -> Result<Vec<GenOutcome>, GenError> {
    move_module(journal, plan)?;
    leave_app(journal, plan)?;
    write_new_app(journal, plan)?;
    add_members(journal, plan)?;
    write_proxy_table(journal, plan)?;
    let mut generated = regenerated(&plan.to, GenMode::Write)?;
    generated.extend(regenerated(&plan.root, GenMode::Write)?);
    Ok(generated)
}

fn plan(
    app: &App,
    module: &Module,
    root: &Path,
    workspace: PathBuf,
    orphaned_tests: Vec<PathBuf>,
) -> SplitPlan {
    let name = module.krate.crate_name.clone();
    let package = module.krate.package.clone();
    let contract = module
        .contract
        .as_ref()
        .map_or_else(String::new, |krate| krate.crate_name.clone());
    let stays_local = module
        .contract_traits
        .iter()
        .filter(|contract_trait| !contract_trait.crosses_apps())
        .flat_map(|contract_trait| {
            contract_trait.methods.iter().filter_map(move |method| {
                let reason = method.stays_local?;
                Some(format!(
                    "{}::{}: {reason}",
                    contract_trait.name, method.name
                ))
            })
        })
        .collect();
    let (calls_back, reaches) = called_modules(app, module);
    let (proxy, shared_prefixes) = proxy_prefixes(app, module);
    SplitPlan {
        contracts: crossing_traits(&module.contract_traits)
            .map(|contract_trait| format!("{contract}::{}", contract_trait.path.join("::")))
            .collect(),
        stays_local,
        lookups: exposed_aggregates(module)
            .map(|aggregate| format!("{contract}::{}", aggregate.name))
            .collect(),
        events: module
            .events
            .iter()
            .map(|event| format!("{name}.{event}"))
            .collect(),
        calls_back,
        reaches,
        proxy,
        shared_prefixes,
        orphaned_tests,
        last_module: app.modules.len() == 1,
        app: app.app.package.clone(),
        new_app: format!("{package}-app"),
        from: module.krate.path.clone(),
        to: root.join("split").join(&package),
        root: root.to_path_buf(),
        workspace,
        module: name,
        package,
    }
}

/// Contracts the module depends on: of modules staying behind, or of ones already split off.
fn called_modules(app: &App, module: &Module) -> (Vec<String>, Vec<String>) {
    let dependencies = module
        .krate
        .dependencies
        .iter()
        .chain(module.contract.iter().flat_map(|krate| &krate.dependencies));
    let mut calls_back = Vec::new();
    let mut reaches = Vec::new();
    for dependency in dependencies {
        let staying = app.modules.iter().find(|other| {
            other.krate.package != module.krate.package
                && other
                    .contract
                    .as_ref()
                    .is_some_and(|krate| krate.package == dependency.package)
        });
        let remote = app
            .split
            .remote
            .iter()
            .find(|remote| remote.contract.package == dependency.package);
        if let Some(staying) = staying {
            calls_back.push(staying.krate.crate_name.clone());
        } else if let Some(remote) = remote {
            reaches.push(remote.name.clone());
        }
    }
    calls_back.sort();
    calls_back.dedup();
    reaches.sort();
    reaches.dedup();
    (calls_back, reaches)
}

/// Each route's path up to its first parameter: the prefixes a reverse proxy sends to the new app.
fn proxy_prefixes(app: &App, module: &Module) -> (Vec<String>, Vec<String>) {
    let sites = app.route_sites();
    let static_prefix = |path: &str| {
        let prefix = path.split('{').next().unwrap_or_default();
        let trimmed = prefix.trim_end_matches('/');
        if trimmed.is_empty() { "/" } else { trimmed }.to_owned()
    };
    let mut prefixes: Vec<String> = sites
        .iter()
        .filter(|site| site.module.krate.package == module.krate.package)
        .filter_map(|site| site.full_path(app))
        .map(|path| static_prefix(&path))
        .collect();
    prefixes.sort();
    prefixes.dedup();
    let staying: Vec<String> = sites
        .iter()
        .filter(|site| site.module.krate.package != module.krate.package)
        .filter_map(|site| site.full_path(app))
        .collect();
    let shared = prefixes
        .iter()
        .filter(|prefix| staying.iter().any(|path| path.starts_with(prefix.as_str())))
        .cloned()
        .collect();
    (prefixes, shared)
}

impl SplitPlan {
    fn shown(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    fn moved_module(&self) -> PathBuf {
        self.to.join("modules").join(&self.package)
    }

    fn contract_package(&self) -> String {
        contract_package(&self.package)
    }

    /// What stops compiling in the app the module leaves.
    fn write_breakage(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.stays_local.is_empty() {
            writeln!(
                formatter,
                "Cannot cross apps, so callers in {} no longer compile",
                self.app
            )?;
            list(formatter, "method", &self.stays_local)?;
        }
        if !self.orphaned_tests.is_empty() {
            writeln!(
                formatter,
                "Tests naming `{}`, which {} no longer depends on: move them to {} \
                 or reach the module through `{}`",
                self.module,
                self.app,
                self.new_app,
                self.contract_package()
            )?;
            for test in &self.orphaned_tests {
                writeln!(formatter, "  test {}", self.shown(test))?;
            }
        }
        Ok(())
    }
}

impl fmt::Display for SplitPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let module = &self.module;
        writeln!(
            formatter,
            "sofie split {module}: module `{module}` leaves `{}` for a new app `{}`.\n",
            self.app, self.new_app
        )?;
        writeln!(formatter, "Moves")?;
        writeln!(
            formatter,
            "  {} → {}: the crate pair, its areas, migrations, translations, config and \
             .env.example",
            self.shown(&self.from),
            self.shown(&self.moved_module())
        )?;
        writeln!(formatter, "Writes")?;
        writeln!(
            formatter,
            "  {}/app: the new app, serving `{module}` (`[package.metadata.sofie] serves`)",
            self.shown(&self.to)
        )?;
        writeln!(formatter, "  {PROXY_TABLE}: the reverse-proxy table")?;
        writeln!(formatter, "Changes")?;
        writeln!(
            formatter,
            "  app/Cargo.toml: drops `{}`, reaches `{}` at its new path, `remote = [\"{module}\"]`, \
             sofie feature `{FEATURE}`",
            self.package,
            self.contract_package()
        )?;
        writeln!(
            formatter,
            "  {}: the new app's crates join the workspace",
            self.workspace.join("Cargo.toml").display()
        )?;
        writeln!(formatter, "  then `sofie gen` in both apps")?;
        writeln!(formatter, "Served by {} over HTTP", self.new_app)?;
        list(formatter, "contract", &self.contracts)?;
        list(formatter, "lookup", &self.lookups)?;
        list(formatter, "event", &self.events)?;
        if !self.calls_back.is_empty() {
            writeln!(formatter, "Served back by {}", self.app)?;
            list(formatter, "module", &self.calls_back)?;
        }
        if !self.reaches.is_empty() {
            writeln!(formatter, "Reached by {} in their own apps", self.new_app)?;
            list(formatter, "module", &self.reaches)?;
        }
        self.write_breakage(formatter)?;
        writeln!(formatter, "Reverse proxy (longest prefix wins)")?;
        for prefix in &self.proxy {
            writeln!(formatter, "  {prefix} → {}", self.new_app)?;
        }
        writeln!(formatter, "  anything else → {}", self.app)?;
        for prefix in &self.shared_prefixes {
            writeln!(
                formatter,
                "  warning: {prefix} also starts a route {} keeps; list that route's path \
                 in {PROXY_TABLE} by hand",
                self.app
            )?;
        }
        writeln!(formatter, "Keys")?;
        let remote = remote_prefix(module);
        writeln!(
            formatter,
            "  {}: {remote}_BASE_URL, and {remote}_PRIVATE=true inside the network",
            self.app
        )?;
        for called in self.calls_back.iter().chain(&self.reaches) {
            let called = remote_prefix(called);
            writeln!(formatter, "  {}: {called}_BASE_URL", self.new_app)?;
        }
        writeln!(
            formatter,
            "  both: the same SOFIE_SIGNING_KEYS; {}: a DATABASE_URL of its own",
            self.new_app
        )?;
        if self.last_module {
            writeln!(
                formatter,
                "Note: `{module}` is the app's last module, so its `modules/*` members leave the \
                 workspace: Cargo refuses a member glob matching nothing"
            )?;
        }
        Ok(())
    }
}

fn list(formatter: &mut fmt::Formatter<'_>, kind: &str, items: &[String]) -> fmt::Result {
    for item in items {
        writeln!(formatter, "  {kind} {item}")?;
    }
    Ok(())
}

fn move_module(journal: &mut Journal, plan: &SplitPlan) -> Result<(), GenError> {
    let to = plan.moved_module();
    journal.rename(&plan.from, &to)?;
    for manifest in [
        to.join("Cargo.toml"),
        to.join("contract").join("Cargo.toml"),
    ] {
        if manifest.is_file() {
            let old = manifest_directory(&plan.from, &to, &manifest);
            let text = rebased_paths(&read(&manifest)?, &old, &manifest_parent(&manifest), plan);
            journal.write(&manifest, text)?;
        }
    }
    Ok(())
}

fn manifest_parent(manifest: &Path) -> PathBuf {
    manifest.parent().map(Path::to_path_buf).unwrap_or_default()
}

fn manifest_directory(from: &Path, to: &Path, manifest: &Path) -> PathBuf {
    let within = manifest_parent(manifest);
    let relative = within.strip_prefix(to).unwrap_or(Path::new(""));
    from.join(relative)
}

/// A path out of the moved module still names the same crate from its new place.
fn rebased_paths(text: &str, old: &Path, new: &Path, plan: &SplitPlan) -> String {
    edit_paths(text, |written| {
        let target = normalized(&old.join(written));
        if target.starts_with(&plan.from) {
            return None;
        }
        relative_path(new, &target)
    })
}

/// Rewrites every `path = "…"` whose replacement `rewrite` gives.
fn edit_paths(text: &str, rewrite: impl Fn(&str) -> Option<String>) -> String {
    const KEY: &str = "path = \"";
    let mut edited = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(KEY) {
        let value_start = start + KEY.len();
        let Some(length) = rest[value_start..].find('"') else {
            break;
        };
        let written = &rest[value_start..value_start + length];
        edited.push_str(&rest[..value_start]);
        match rewrite(written) {
            Some(replaced) => edited.push_str(&toml_escaped(&replaced)),
            None => edited.push_str(written),
        }
        rest = &rest[value_start + length..];
    }
    edited.push_str(rest);
    edited
}

fn normalized(path: &Path) -> PathBuf {
    let mut parts: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir if matches!(parts.last(), Some(Component::Normal(_))) => {
                parts.pop();
            }
            Component::CurDir => {}
            other => parts.push(other),
        }
    }
    parts.iter().collect()
}

fn leave_app(journal: &mut Journal, plan: &SplitPlan) -> Result<(), GenError> {
    let old_contract = plan.from.join("contract");
    let new_contract = plan.moved_module().join("contract");
    for manifest in staying_manifests(plan)? {
        let directory = manifest_parent(&manifest);
        let text = edit_paths(&read(&manifest)?, |written| {
            (normalized(&directory.join(written)) == old_contract)
                .then(|| relative_path(&directory, &new_contract))
                .flatten()
        });
        journal.write(&manifest, text)?;
    }
    let app_manifest = plan.root.join("app").join("Cargo.toml");
    let mut text = without_dependency(&read(&app_manifest)?, &plan.package);
    let depends = toml_section(&text, DEPENDENCIES).is_some_and(|(start, end)| {
        dependency_line_start(&text[start..end], &plan.contract_package()).is_some()
    });
    if !depends {
        let path = relative_path(&plan.root.join("app"), &new_contract).unwrap_or_default();
        text = with_dependency_line(&text, &path_dependency(&plan.contract_package(), &path));
    }
    text = with_sofie_feature(&text);
    text = with_metadata(&text, "remote", std::slice::from_ref(&plan.module));
    text = with_metadata(&text, "serves", &plan.calls_back);
    journal.write(&app_manifest, text)
}

/// The app's and every earlier split's: any of them may reach the moved contract.
fn staying_manifests(plan: &SplitPlan) -> Result<Vec<PathBuf>, GenError> {
    let subdirectories =
        |directory: &Path| sofie_model::subdirectories(directory).map_err(GenError::Model);
    let mut apps = vec![plan.root.clone()];
    apps.extend(subdirectories(&plan.root.join("split"))?);
    let mut manifests = Vec::new();
    for app in apps.iter().filter(|app| **app != plan.to) {
        manifests.push(app.join("app").join("Cargo.toml"));
        for module in subdirectories(&app.join("modules"))? {
            manifests.push(module.join("Cargo.toml"));
            manifests.push(module.join("contract").join("Cargo.toml"));
        }
    }
    manifests.retain(|manifest| manifest.is_file());
    Ok(manifests)
}

/// Its manifest follows the app it leaves: the same package keys, `sofie` dependency,
/// build dependencies and lints, each path rebased.
fn write_new_app(journal: &mut Journal, plan: &SplitPlan) -> Result<(), GenError> {
    let app = plan.to.join("app");
    let left = plan.root.join("app");
    let original = edit_paths(&read(&left.join("Cargo.toml"))?, |written| {
        relative_path(&app, &normalized(&left.join(written)))
    });
    let path = |target: &Path| relative_path(&app, target).unwrap_or_default();
    let module = plan.moved_module();
    let mut dependencies = kept_lines(&original, DEPENDENCIES, |line| {
        line.starts_with("sofie ") || line.starts_with("sofie.")
    });
    dependencies.push_str(&path_dependency(&plan.package, &path(&module)));
    dependencies.push_str(&path_dependency(
        &plan.contract_package(),
        &path(&module.join("contract")),
    ));
    let remote: Vec<String> = plan
        .calls_back
        .iter()
        .chain(&plan.reaches)
        .cloned()
        .collect();
    for called in &remote {
        let contract = contract_directory(plan, called);
        dependencies.push_str(&path_dependency(
            &contract_package(called),
            &path(&contract),
        ));
    }
    let package = kept_lines(&original, "[package]", |line| {
        !line.starts_with("name") && !line.starts_with("description")
    });
    let mut manifest = format!(
        "[package]\nname = \"{}\"\n{}\n\n[package.metadata.sofie]\nserves = [\"{}\"]\nremote = [{}]\n\n\
         [dependencies]\n{dependencies}",
        plan.new_app,
        package.trim_end(),
        plan.module,
        quoted(&remote),
    );
    for kept in ["[build-dependencies]", "[lints]"] {
        let body = section(&original, kept);
        if !body.is_empty() {
            let _ = write!(manifest, "\n{kept}\n{}\n", body.trim_end());
        }
    }
    let crate_name = sofie_model::naming::crate_name(&plan.new_app);
    let [build, library, main] = app_sources(&crate_name);
    let mut files = vec![("app/Cargo.toml", manifest), library, main];
    if left.join("build.rs").is_file() {
        files.push(build);
    }
    write_all(journal, &plan.to, &files)
}

fn section<'text>(text: &'text str, header: &str) -> &'text str {
    toml_section(text, header).map_or("", |(start, end)| &text[start..end])
}

fn kept_lines(text: &str, header: &str, keep: impl Fn(&str) -> bool) -> String {
    section(text, header)
        .lines()
        .filter(|line| keep(line))
        .fold(String::new(), |mut kept, line| {
            let _ = writeln!(kept, "{line}");
            kept
        })
}

/// Where a called module's contract lives: staying behind, or in the app an earlier split made.
fn contract_directory(plan: &SplitPlan, module: &str) -> PathBuf {
    let staying = plan.root.join("modules").join(module).join("contract");
    if staying.is_dir() {
        return staying;
    }
    plan.root
        .join("split")
        .join(module)
        .join("modules")
        .join(module)
        .join("contract")
}

const APP_MEMBERS: [&str; 3] = ["app", "modules/*", "modules/*/contract"];

fn add_members(journal: &mut Journal, plan: &SplitPlan) -> Result<(), GenError> {
    let manifest = plan.workspace.join("Cargo.toml");
    let mut text = read(&manifest)?;
    if plan.last_module {
        for member in &APP_MEMBERS[1..] {
            text = without_member(&text, &workspace_member(plan, &plan.root, member));
        }
    }
    let members = APP_MEMBERS
        .map(|member| {
            format!(
                "\"{}\", ",
                toml_escaped(&workspace_member(plan, &plan.to, member))
            )
        })
        .concat();
    let Some(start) = text.find("members = [") else {
        return Err(GenError::Usage(format!(
            "{} lists no `members = [`; add the new app's crates to the workspace by hand",
            manifest.display()
        )));
    };
    let at = start + "members = [".len();
    journal.write(
        &manifest,
        format!("{}{members}{}", &text[..at], &text[at..]),
    )
}

fn workspace_member(plan: &SplitPlan, app: &Path, member: &str) -> String {
    match relative_path(&plan.workspace, app)
        .unwrap_or_default()
        .as_str()
    {
        "" => member.to_owned(),
        within => format!("{within}/{member}"),
    }
}

/// Its line goes too when nothing else is on it.
fn without_member(text: &str, member: &str) -> String {
    let quoted = format!("\"{}\"", toml_escaped(member));
    let Some(start) = text.find(&quoted) else {
        return text.to_owned();
    };
    let after = &text[start + quoted.len()..];
    let end = text.len() - after.trim_start_matches([',', ' ']).len();
    let line_start = text[..start].rfind('\n').map_or(0, |newline| newline + 1);
    if text[line_start..start].trim().is_empty() && text[end..].starts_with('\n') {
        return format!("{}{}", &text[..line_start], &text[end + 1..]);
    }
    format!("{}{}", &text[..start], &text[end..])
}

/// Written beside the app it leaves; a later split adds its prefixes to the same table.
fn write_proxy_table(journal: &mut Journal, plan: &SplitPlan) -> Result<(), GenError> {
    let table = plan.root.join(PROXY_TABLE);
    let mut entries: Vec<String> = read_existing(&table)?
        .unwrap_or_default()
        .lines()
        .filter(|line| line.starts_with('"'))
        .map(str::to_owned)
        .collect();
    entries.extend(
        plan.proxy
            .iter()
            .map(|prefix| format!("\"{}\" = \"{}\"", toml_escaped(prefix), plan.new_app)),
    );
    let text = format!(
        "# Written by `sofie split`: the app answering each path, the longest matching\n\
         # prefix first; any other path goes to `default`. `/_sofie/` stays inside the\n\
         # network: only apps call it, with principal tokens.\n\
         default = \"{}\"\n\n[prefixes]\n{}\n",
        plan.app,
        entries.join("\n")
    );
    journal.write(&table, text)
}

fn quoted(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

fn without_dependency(text: &str, package: &str) -> String {
    let Some((start, end)) = toml_section(text, DEPENDENCIES) else {
        return text.to_owned();
    };
    let kept: String = text[start..end]
        .split_inclusive('\n')
        .filter(|line| {
            let key = line.split(['=', '.']).next().unwrap_or_default().trim();
            key != package
        })
        .collect();
    format!("{}{kept}{}", &text[..start], &text[end..])
}

fn with_sofie_feature(text: &str) -> String {
    let Some((start, end)) = toml_section(text, DEPENDENCIES) else {
        return text.to_owned();
    };
    let edited: String = text[start..end]
        .split_inclusive('\n')
        .map(|line| {
            let trimmed = line.trim_end();
            if trimmed == "sofie.workspace = true" {
                return format!("sofie = {{ workspace = true, features = [\"{FEATURE}\"] }}\n");
            }
            if !trimmed.starts_with("sofie = {") || trimmed.contains(FEATURE) {
                return line.to_owned();
            }
            match trimmed.find("features = [") {
                Some(at) => {
                    let at = at + "features = [".len();
                    format!("{}\"{FEATURE}\", {}\n", &trimmed[..at], &trimmed[at..])
                }
                None => format!(
                    "{}, features = [\"{FEATURE}\"] }}\n",
                    trimmed.trim_end_matches('}').trim_end()
                ),
            }
        })
        .collect();
    format!("{}{edited}{}", &text[..start], &text[end..])
}

/// Adds `names` to `key` under `[package.metadata.sofie]`, the table `sofie_model` reads.
fn with_metadata(text: &str, key: &str, names: &[String]) -> String {
    const TABLE: &str = "[package.metadata.sofie]";
    if names.is_empty() {
        return text.to_owned();
    }
    let Some((table, end)) = toml_section(text, TABLE) else {
        let package_end =
            toml_table(text, DEPENDENCIES).map_or(text.len(), |(header_start, ..)| header_start);
        return format!(
            "{}{TABLE}\n{key} = [{}]\n\n{}",
            &text[..package_end],
            quoted(names),
            &text[package_end..]
        );
    };
    let prefix = format!("{key} = [");
    match text[table..end].find(&prefix) {
        Some(offset) => {
            let at = table + offset + prefix.len();
            let listed = format!("{}, ", quoted(names));
            format!("{}{listed}{}", &text[..at], &text[at..])
        }
        None => format!(
            "{}{key} = [{}]\n{}",
            &text[..table],
            quoted(names),
            &text[table..]
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_names_a_crate_by_a_path_starting_with_it() {
        assert!(names_crate("use order::Order;", "order"));
        assert!(names_crate("let total = <order::Order>::total;", "order"));
        assert!(!names_crate("let order = place(&shop);", "order"));
        assert!(!names_crate("use crate::order::Order;", "order"));
        assert!(!names_crate("use reorder::Order;", "order"));
        assert!(!names_crate("use order_contract::routes;", "order"));
    }

    const MANIFEST: &str = "[package]\nname = \"shop\"\n\n[dependencies]\nsofie.workspace = true\n\
                            order = { path = \"../modules/order\" }\n\
                            order-contract = { path = \"../modules/order/contract\" }\n\n\
                            [lints]\nworkspace = true\n";

    #[test]
    fn a_metadata_key_goes_into_its_own_table_only() {
        let text = "[package.metadata.sofie]\nserves = [\"a\"]\n\n[package.metadata.other]\n\
                    remote = [\"x\"]\n";

        let text = with_metadata(text, "remote", &["order".to_owned()]);

        assert_eq!(
            text,
            "[package.metadata.sofie]\nremote = [\"order\"]\nserves = [\"a\"]\n\n\
             [package.metadata.other]\nremote = [\"x\"]\n"
        );
    }

    #[test]
    fn the_app_left_drops_the_module_and_turns_on_remote_contracts() {
        let text = without_dependency(MANIFEST, "order");
        let text = with_sofie_feature(&text);
        let text = with_metadata(&text, "remote", &["order".to_owned()]);
        let text = with_metadata(&text, "remote", &["billing".to_owned()]);

        assert_eq!(
            text,
            "[package]\nname = \"shop\"\n\n[package.metadata.sofie]\nremote = [\"billing\", \"order\"]\n\n\
             [dependencies]\nsofie = { workspace = true, features = [\"remote-contracts\"] }\n\
             order-contract = { path = \"../modules/order/contract\" }\n\n[lints]\nworkspace = true\n"
        );
    }

    #[test]
    fn an_sofie_dependency_with_features_gains_one_more() {
        let text = "[dependencies]\nsofie = { workspace = true, features = [\"config\"] }\n";

        assert_eq!(
            with_sofie_feature(text),
            "[dependencies]\nsofie = { workspace = true, features = [\"remote-contracts\", \"config\"] }\n"
        );
    }

    #[test]
    fn a_member_leaves_the_workspace_with_its_line_when_it_had_one() {
        let listed = "members = [\n    \"shop/app\",\n    \"shop/modules/*\",\n]\n";
        let inline = "members = [\"app\", \"modules/*\", \"modules/*/contract\"]\n";

        assert_eq!(
            without_member(listed, "shop/modules/*"),
            "members = [\n    \"shop/app\",\n]\n"
        );
        assert_eq!(
            without_member(inline, "modules/*"),
            "members = [\"app\", \"modules/*/contract\"]\n"
        );
        assert_eq!(without_member(inline, "other/*"), inline);
    }

    #[test]
    fn a_proxy_table_it_cannot_read_is_left_as_it_is() {
        let directory = sofie_model::testing::TemporaryDirectory::new("sofie-split-proxy");
        let table = directory.path(PROXY_TABLE);
        fs::write(&table, b"\"/orders\" = \"order-app\"\n\xff\n").unwrap();
        let plan = SplitPlan {
            module: "billing".to_owned(),
            package: "billing".to_owned(),
            app: "shop".to_owned(),
            new_app: "billing-app".to_owned(),
            root: directory.root().to_path_buf(),
            from: PathBuf::new(),
            to: PathBuf::new(),
            workspace: PathBuf::new(),
            contracts: Vec::new(),
            stays_local: Vec::new(),
            lookups: Vec::new(),
            events: Vec::new(),
            calls_back: Vec::new(),
            reaches: Vec::new(),
            proxy: vec!["/invoices".to_owned()],
            shared_prefixes: Vec::new(),
            orphaned_tests: Vec::new(),
            last_module: false,
        };

        assert!(write_proxy_table(&mut Journal::default(), &plan).is_err());
        assert_eq!(
            fs::read(&table).unwrap(),
            b"\"/orders\" = \"order-app\"\n\xff\n"
        );
    }

    #[test]
    fn paths_are_rewritten_only_where_asked() {
        let text = "a = { path = \"contract\" }\nb = { path = \"../../crates/sofie-identity\" }\n";

        let edited = edit_paths(text, |written| {
            written
                .starts_with("..")
                .then(|| format!("../../{written}"))
        });

        assert_eq!(
            edited,
            "a = { path = \"contract\" }\nb = { path = \"../../../../crates/sofie-identity\" }\n"
        );
        assert_eq!(
            normalized(Path::new("/shop/modules/order/../../crates")),
            Path::new("/shop/crates")
        );
    }
}
