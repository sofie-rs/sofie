use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use sofie_core::{Error, Result, unpoisoned};
use sofie_db::File;
use sofie_engine::{Components, Template, parse_template_with_recorded};

use super::{FormDescription, Screen, Screens, describe};

/// Reread when its file's or a module component's modification time changes.
pub(super) struct TemplateFile {
    key: String,
    path: PathBuf,
    /// What the build script compiled, whose `t(…)` texts a build without `i18n` writes.
    compiled: &'static [u8],
    loaded: Mutex<Loaded>,
    descriptions: Vec<FormDescription>,
    components: ComponentFiles,
}

pub(super) type ComponentFiles = Arc<[(String, PathBuf)]>;

struct Loaded {
    modified: SystemTime,
    template: Arc<Template>,
}

pub(super) fn add_wired(screens: &mut Screens, key: &str, file: &File) -> Result<()> {
    let path = Path::new(file.path);
    screens.refuse_duplicate(key)?;
    let file = TemplateFile {
        key: key.to_owned(),
        path: path.to_path_buf(),
        compiled: file.bytes,
        loaded: Mutex::new(load(key, path, file.bytes, &[], &screens.component_files)?),
        descriptions: Vec::new(),
        components: Arc::clone(&screens.component_files),
    };
    screens.by_key.insert(key.to_owned(), Screen::File(file));
    Ok(())
}

impl TemplateFile {
    pub(super) fn describe(&mut self, description: FormDescription) -> &mut Arc<Template> {
        self.descriptions.push(description);
        let loaded = self.loaded.get_mut();
        &mut loaded.unwrap_or_else(PoisonError::into_inner).template
    }

    /// The lock is held across the check and the reread, so requests never race on one file.
    pub(super) fn current(&self) -> Result<Arc<Template>> {
        let components = &self.components;
        let mut loaded = unpoisoned::lock(&self.loaded);
        if latest(&self.key, &self.path, components)? != loaded.modified {
            *loaded = load(
                &self.key,
                &self.path,
                self.compiled,
                &self.descriptions,
                components,
            )?;
        }
        Ok(Arc::clone(&loaded.template))
    }
}

/// The time is taken before reading, so a write during the read is picked up next time.
fn load(
    key: &str,
    path: &Path,
    compiled: &[u8],
    descriptions: &[FormDescription],
    components: &[(String, PathBuf)],
) -> Result<Loaded> {
    let modified = latest(key, path, components)?;
    let read = |path: &Path| {
        fs::read_to_string(path).map_err(|error| {
            Error::internal(
                format!("reading screen {key} from {}", path.display()),
                error,
            )
        })
    };
    let source = read(path)?;
    let mut written = Components::default();
    for (name, file) in components {
        written
            .add(name, read(file)?)
            .map_err(|error| Error::internal(format!("adding component {name}"), error))?;
    }
    let mut template = parse_template_with_recorded(key, &source, &written, compiled)
        .map_err(|error| Error::internal(format!("parsing screen {key}"), error))?;
    for description in descriptions {
        describe(key, &mut template, description)?;
    }
    Ok(Loaded {
        modified,
        template: Arc::new(template),
    })
}

fn latest(key: &str, path: &Path, components: &[(String, PathBuf)]) -> Result<SystemTime> {
    components
        .iter()
        .map(|(_, file)| modified(key, file))
        .try_fold(modified(key, path)?, |latest, time| Ok(latest.max(time?)))
}

fn modified(key: &str, path: &Path) -> Result<SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| {
            Error::internal(
                format!(
                    "reading the modification time of screen {key} at {}",
                    path.display()
                ),
                error,
            )
        })
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::time::Duration;

    use serde_json::json;
    use sofie_values::{FormField, FormFields};

    use super::*;

    fn rendered(screens: &Screens, key: &str) -> String {
        screens
            .template(key)
            .unwrap()
            .render(&json!({"a": 1}))
            .unwrap()
    }

    fn from_file(key: &'static str, path: &Path) -> Screens {
        wired(key, path, &[])
    }

    fn wired(key: &'static str, path: &Path, components: &[(&str, &str)]) -> Screens {
        let module = crate::Module {
            screens: Vec::leak(vec![(key, unread(path))]),
            ..crate::Module::new("order")
        };
        super::super::wired_screens(&[module], components).unwrap()
    }

    fn unread(path: &Path) -> sofie_db::File {
        let path = path.to_str().unwrap().to_owned();
        let compiled = sofie_engine::compile_template("built", "").unwrap();
        sofie_db::File::new(String::leak(path), Vec::leak(compiled))
    }

    /// Dated later, as a file system with a coarse clock would not tell the write apart.
    fn rewrite(path: &Path, source: &str) {
        fs::write(path, source).unwrap();
        let later = SystemTime::now() + Duration::from_secs(5);
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(later)
            .unwrap();
    }

    struct Contact;

    impl FormFields for Contact {
        const FIELDS: &'static [FormField] = &[FormField {
            name: "email",
            input: sofie_values::Input::typed(&[("type", "email")]),
            required: true,
            constraints: &[],
        }];
    }

    #[test]
    fn a_described_form_keeps_its_fields_when_its_file_is_read_again() {
        let path =
            std::env::temp_dir().join(format!("sofie-http-form-{}.html", std::process::id()));
        let form =
            |label: &str| format!("<Form id=\"c\" post=\"/c\"><Submit label=\"{label}\" /></Form>");
        fs::write(&path, form("Send")).unwrap();
        let mut screens = from_file("crm/web/contact", &path);
        screens
            .describe_form::<Contact>("crm/web/contact", "c")
            .unwrap();
        assert!(rendered(&screens, "crm/web/contact").contains("type=\"email\""));

        rewrite(&path, &form("Go"));

        let html = rendered(&screens, "crm/web/contact");
        assert!(
            html.contains("type=\"email\"") && html.contains(">Go<"),
            "{html}"
        );
        assert!(
            screens
                .describe_form::<Contact>("crm/web/none", "c")
                .is_err()
        );
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_changed_file_is_read_again() {
        let path =
            std::env::temp_dir().join(format!("sofie-http-screen-{}.html", std::process::id()));
        fs::write(&path, "<p>old {data.a}</p>").unwrap();
        let screens = from_file("order/web/orders", &path);
        assert_eq!(rendered(&screens, "order/web/orders"), "<p>old 1</p>");

        rewrite(&path, "<p>new {data.a}</p>");

        assert_eq!(rendered(&screens, "order/web/orders"), "<p>new 1</p>");
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn an_edited_component_is_read_again_with_the_screens_writing_it() {
        let directory =
            std::env::temp_dir().join(format!("sofie-http-card-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let (screen, card) = (
            directory.join("orders.html"),
            directory.join("OrderCard.html"),
        );
        fs::write(&screen, "<OrderCard value={data.a} />").unwrap();
        fs::write(&card, "<b>{data.value}</b>").unwrap();
        let screens = wired(
            "order/web/orders",
            &screen,
            &[("OrderCard", card.to_str().unwrap())],
        );
        assert_eq!(rendered(&screens, "order/web/orders"), "<b>1</b>");

        rewrite(&card, "<i>{data.value}</i>");

        assert_eq!(rendered(&screens, "order/web/orders"), "<i>1</i>");
        fs::remove_dir_all(&directory).unwrap();
    }
}
