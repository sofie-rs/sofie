use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

use sofie_engine::{ActionPost, Binding, Components, Template, TemplateError};
use sofie_model::Module;

/// Read and parsed once per run, then borrowed by every rule and generator.
pub struct Screens {
    by_file: HashMap<PathBuf, io::Result<Screen>>,
    components: Components,
    /// A `web/` component that cannot be read or added, with why.
    refused_components: Vec<(PathBuf, String)>,
}

pub struct Screen {
    pub source: String,
    pub template: Result<Template, TemplateError>,
    pub bindings: Result<Vec<Binding>, TemplateError>,
    pub actions: Result<Vec<ActionPost>, TemplateError>,
}

impl Screens {
    #[must_use]
    pub fn read(modules: &[Module]) -> Self {
        let (components, refused_components) = read_components(modules);
        let files = modules
            .iter()
            .flat_map(|module| &module.areas)
            .flat_map(|area| {
                let overrides = area.overrides.iter().map(|replacement| &replacement.file);
                area.screens.iter().chain(overrides)
            });
        let by_file = files
            .map(|file| (file.clone(), Screen::read(file, &components)))
            .collect();
        Self {
            by_file,
            components,
            refused_components,
        }
    }

    /// # Panics
    ///
    /// When `file` is no screen or override of the app the screens were read from.
    #[must_use]
    pub fn screen(&self, file: &Path) -> &io::Result<Screen> {
        self.by_file.get(file).unwrap_or_else(|| {
            panic!(
                "{} is not a screen of the app the screens were read from",
                file.display()
            )
        })
    }

    #[must_use]
    pub fn components(&self) -> &Components {
        &self.components
    }

    #[must_use]
    pub fn refused_components(&self) -> &[(PathBuf, String)] {
        &self.refused_components
    }
}

fn read_components(modules: &[Module]) -> (Components, Vec<(PathBuf, String)>) {
    let mut components = Components::default();
    let mut refused = Vec::new();
    for file in modules.iter().flat_map(|module| &module.components) {
        let name = file
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let added = fs::read_to_string(file)
            .map_err(|error| format!("the component cannot be read: {error}"))
            .and_then(|source| components.add(&name, source).map_err(|error| error.message));
        if let Err(message) = added {
            refused.push((file.clone(), message));
        }
    }
    (components, refused)
}

impl Screen {
    fn read(file: &Path, components: &Components) -> io::Result<Self> {
        let source = fs::read_to_string(file)?;
        let name = file.display().to_string();
        Ok(Self {
            template: Template::parse_with(&name, &source, components),
            bindings: sofie_engine::bindings_with(&name, &source, components),
            actions: sofie_engine::action_posts_with(&name, &source, components),
            source,
        })
    }
}
