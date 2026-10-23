#[cfg(feature = "development")]
mod development;

#[cfg(feature = "development")]
use development::add_wired;

use std::collections::HashMap;
use std::sync::Arc;

use sofie_core::{Error, Result};
use sofie_db::File;
use sofie_engine::{Template, load_template};
use sofie_values::{FormField, FormFields};

use crate::Module;

/// Keyed `<module>/<area>/<name>`.
#[derive(Default)]
pub struct Screens {
    by_key: HashMap<String, Screen>,
    #[cfg(feature = "i18n")]
    pub(crate) translations: Option<sofie_engine::Translations>,
    #[cfg(feature = "islands")]
    pub(crate) islands: crate::islands::Islands,
    #[cfg(feature = "clients")]
    pub(crate) clients: Vec<sofie_engine::ClientBundle>,
    #[cfg(feature = "development")]
    component_files: development::ComponentFiles,
}

enum Screen {
    Embedded(Arc<Template>),
    #[cfg(feature = "development")]
    File(development::TemplateFile),
}

type FormDescription = (String, &'static [FormField]);

fn describe(key: &str, template: &mut Template, (form, fields): &FormDescription) -> Result<()> {
    template
        .describe_form(form, fields)
        .map_err(|error| Error::internal(format!("describing screen {key}"), error))
}

/// With `development`, each template is read and parsed again whenever it or one of the module
/// `components` (`(name, path)`) is edited; otherwise its compiled program is loaded. A screen
/// keyed under another module than the one listing it replaces that module's own.
pub fn wired_screens(modules: &[Module], components: &[(&str, &str)]) -> Result<Screens> {
    let mut screens = Screens::default();
    #[cfg(feature = "development")]
    {
        screens.component_files = components
            .iter()
            .map(|(name, file)| ((*name).to_owned(), std::path::PathBuf::from(file)))
            .collect();
    }
    #[cfg(not(feature = "development"))]
    let _ = components;
    for (key, file) in listed(modules, false) {
        add_wired(&mut screens, key, file)?;
    }
    for (key, file) in listed(modules, true) {
        if !screens.contains(key) {
            add_wired(&mut screens, key, file)?;
        }
    }
    for module in modules {
        screens.describe_forms(module.forms)?;
    }
    Ok(screens)
}

fn listed(modules: &[Module], own: bool) -> impl Iterator<Item = &(&'static str, File)> {
    modules.iter().flat_map(move |module| {
        module
            .screens
            .iter()
            .filter(move |(key, _)| (key.split('/').next() == Some(module.name)) == own)
    })
}

#[derive(Clone, Copy)]
pub struct Form {
    screen: &'static str,
    id: &'static str,
    fields: &'static [FormField],
}

impl Form {
    #[must_use]
    pub const fn of<I: FormFields>(screen: &'static str, id: &'static str) -> Self {
        Self {
            screen,
            id,
            fields: I::FIELDS,
        }
    }
}

#[cfg(not(feature = "development"))]
fn add_wired(screens: &mut Screens, key: &str, file: &File) -> Result<()> {
    screens.add(key, file)
}

#[cfg(feature = "parse")]
pub fn screens_from_sources(sources: &[(&str, &str)]) -> Result<Screens> {
    let mut screens = Screens::default();
    for (key, source) in sources {
        screens.add_source(key, source)?;
    }
    Ok(screens)
}

impl Screens {
    fn add(&mut self, key: &str, file: &File) -> Result<()> {
        self.add_with(key, || {
            load_template(key, file.bytes).map_err(|error| {
                Error::internal(format!("loading screen {key} from {}", file.path), error)
            })
        })
    }

    /// Skips keys the app already gave a template: that is how an app overrides a built-in screen.
    pub fn add_builtin(&mut self, files: &[(&str, File)]) -> Result<()> {
        for (key, file) in files {
            if !self.contains(key) {
                self.add(key, file)?;
            }
        }
        Ok(())
    }

    #[cfg(feature = "parse")]
    pub fn add_source(&mut self, key: &str, source: &str) -> Result<()> {
        self.add_with(key, || {
            parse_with(key, source, &sofie_engine::Components::default())
        })
    }

    pub(crate) fn add_with(
        &mut self,
        key: &str,
        template: impl FnOnce() -> Result<Template>,
    ) -> Result<()> {
        self.refuse_duplicate(key)?;
        self.by_key
            .insert(key.to_owned(), Screen::Embedded(Arc::new(template()?)));
        Ok(())
    }

    fn refuse_duplicate(&self, key: &str) -> Result<()> {
        if !self.contains(key) {
            return Ok(());
        }
        Err(Error::internal_message(format!(
            "screen {key} is added twice; a key names exactly one template"
        )))
    }

    /// A development build gives them again whenever it rereads the file.
    pub fn describe_form<I: FormFields>(&mut self, key: &str, form: &str) -> Result<()> {
        self.describe_fields(key, form, I::FIELDS)
    }

    pub fn describe_forms(&mut self, forms: &[Form]) -> Result<()> {
        for form in forms {
            self.describe_fields(form.screen, form.id, form.fields)?;
        }
        Ok(())
    }

    #[cfg_attr(
        not(feature = "development"),
        expect(
            clippy::manual_let_else,
            reason = "a development build matches files too"
        )
    )]
    pub(crate) fn describe_fields(
        &mut self,
        key: &str,
        form: &str,
        fields: &'static [FormField],
    ) -> Result<()> {
        let description = (form.to_owned(), fields);
        let template = match self.by_key.get_mut(key) {
            Some(Screen::Embedded(template)) => template,
            #[cfg(feature = "development")]
            Some(Screen::File(file)) => file.describe(description.clone()),
            None => return Err(not_loaded(key)),
        };
        describe(key, unshared(key, template)?, &description)
    }

    pub(crate) fn contains(&self, key: &str) -> bool {
        self.by_key.contains_key(key)
    }

    pub(crate) fn template(&self, key: &str) -> Result<Arc<Template>> {
        match self.by_key.get(key) {
            Some(Screen::Embedded(template)) => Ok(Arc::clone(template)),
            #[cfg(feature = "development")]
            Some(Screen::File(file)) => file.current(),
            None => Err(not_loaded(key)),
        }
    }
}

fn not_loaded(key: &str) -> Error {
    Error::internal_message(format!("screen {key} is not loaded"))
}

/// Screens are described while the app is built, before any request holds one.
fn unshared<'t>(key: &str, template: &'t mut Arc<Template>) -> Result<&'t mut Template> {
    Arc::get_mut(template).ok_or_else(|| {
        Error::internal_message(format!(
            "screen {key} is described while a request renders it"
        ))
    })
}

#[cfg(feature = "parse")]
fn parse_with(key: &str, source: &str, components: &sofie_engine::Components) -> Result<Template> {
    Template::parse_with(key, source, components)
        .map_err(|error| Error::internal(format!("parsing screen {key}"), error))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rendered(screens: &Screens, key: &str) -> String {
        screens
            .template(key)
            .unwrap()
            .render(&json!({"a": 1}))
            .unwrap()
    }

    #[test]
    fn a_key_names_one_template() {
        let mut screens = screens_from_sources(&[("order/web/orders", "<p>{data.a}</p>")]).unwrap();

        let error = screens.add_source("order/web/orders", "<p>").unwrap_err();

        assert!(
            error
                .to_string()
                .contains("screen order/web/orders is added twice"),
            "{error}"
        );
        assert_eq!(rendered(&screens, "order/web/orders"), "<p>1</p>");
        assert_eq!(
            screens.template("order/web/orders").unwrap().name(),
            "order/web/orders"
        );
    }

    #[test]
    fn an_app_template_overrides_a_built_in_one_under_its_key() {
        let mut screens =
            screens_from_sources(&[("sofie_identity/web/sign-in", "<p>app</p>")]).unwrap();

        let compiled = |key, source| {
            let program = sofie_engine::compile_template(key, source).unwrap();
            File::new("sign.html", Vec::leak(program))
        };
        let sign_in = compiled("sofie_identity/web/sign-in", "<p>built in</p>");
        let sign_up = compiled("sofie_identity/web/sign-up", "<p>{data.a}</p>");

        screens
            .add_builtin(&[
                ("sofie_identity/web/sign-in", sign_in),
                ("sofie_identity/web/sign-up", sign_up),
            ])
            .unwrap();

        assert_eq!(
            rendered(&screens, "sofie_identity/web/sign-in"),
            "<p>app</p>"
        );
        assert_eq!(rendered(&screens, "sofie_identity/web/sign-up"), "<p>1</p>");
    }

    #[cfg(not(feature = "development"))]
    #[test]
    fn a_module_s_screen_keyed_under_another_module_replaces_that_one() {
        let compiled = |key: &'static str, source: &str| {
            let program = sofie_engine::compile_template(key, source).unwrap();
            (key, File::new("screen.html", Vec::leak(program)))
        };
        let identity = Module {
            screens: Vec::leak(vec![
                compiled("sofie_identity/web/sign-in", "<p>built in</p>"),
                compiled("sofie_identity/web/sign-up", "<p>{data.a}</p>"),
            ]),
            ..Module::new("sofie_identity")
        };
        let order = Module {
            screens: Vec::leak(vec![compiled("sofie_identity/web/sign-in", "<p>app</p>")]),
            ..Module::new("order")
        };

        let screens = wired_screens(&[identity, order], &[]).unwrap();

        assert_eq!(
            rendered(&screens, "sofie_identity/web/sign-in"),
            "<p>app</p>"
        );
        assert_eq!(rendered(&screens, "sofie_identity/web/sign-up"), "<p>1</p>");
    }

    #[test]
    fn a_broken_template_names_its_key() {
        let error = screens_from_sources(&[("order/web/broken", "<p>")])
            .err()
            .unwrap();

        assert!(
            error
                .to_string()
                .starts_with("parsing screen order/web/broken: order/web/broken:1:1"),
            "{error}"
        );
    }

    #[test]
    fn an_unreadable_program_names_its_key() {
        let error = Screens::default()
            .add(
                "order/web/broken",
                &File::new("broken.html", b"not a program"),
            )
            .unwrap_err();

        assert!(
            error
                .to_string()
                .starts_with("loading screen order/web/broken from broken.html"),
            "{error}"
        );
    }
}
