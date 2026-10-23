use std::sync::Arc;

use sofie_core::{Error, Result, Show};
use sofie_db::{Db, File};

use crate::Body;

/// A template the build script compiled, keyed `<crate>/<name>` after its file `mails/<name>.html`.
#[derive(Clone, Copy, Debug)]
pub struct Template {
    key: &'static str,
    file: File,
}

impl Template {
    #[must_use]
    pub const fn new(key: &'static str, file: File) -> Self {
        Self { key, file }
    }

    #[must_use]
    pub fn key(&self) -> &'static str {
        self.key
    }

    /// Loaded per mail: mails are few, and a template is a short read of bytes.
    pub(crate) fn render(&self, data: &(impl Show + ?Sized)) -> Result<Body> {
        let template = self.load()?;
        let html = template.render(data).map_err(|error| self.failed(error))?;
        Ok(Body::rendered(html))
    }

    #[cfg(not(feature = "development"))]
    fn load(&self) -> Result<sofie_engine::Template> {
        sofie_engine::load_template(self.key, self.file.bytes).map_err(|error| self.failed(error))
    }

    #[cfg(feature = "development")]
    fn load(&self) -> Result<sofie_engine::Template> {
        self.parsed_from_disk()
    }

    #[cfg(any(test, feature = "development"))]
    fn parsed_from_disk(&self) -> Result<sofie_engine::Template> {
        let source = std::fs::read_to_string(self.file.path).map_err(|error| {
            Error::internal(
                format!("reading mail `{}` from {}", self.key, self.file.path),
                error,
            )
        })?;
        sofie_engine::parse_template_with_recorded(
            self.key,
            &source,
            &sofie_engine::Components::default(),
            self.file.bytes,
        )
        .map_err(|error| self.failed(error))
    }

    fn failed(&self, error: sofie_engine::TemplateError) -> Error {
        Error::internal(
            format!("mail `{}` from {}", self.key, self.file.path),
            error,
        )
    }
}

pub(crate) struct Registered(pub(crate) &'static [Template]);

pub fn templates(db: &Db, templates: &'static [Template]) {
    sofie_db::extend(db, Arc::new(Registered(templates)));
}

pub(crate) fn find<'a>(registered: &'a [Arc<Registered>], key: &str) -> Option<&'a Template> {
    registered
        .iter()
        .flat_map(|table| table.0)
        .find(|template| template.key == key)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_development_build_reads_the_template_as_it_is_on_disk() {
        let path = std::env::temp_dir().join(format!("sofie-mail-{}.html", std::process::id()));
        std::fs::write(&path, "<p>old {data.name}</p>").unwrap();
        let path: &'static str = String::leak(path.to_str().unwrap().to_owned());
        let template = Template::new(
            "order/receipt",
            File::new(
                path,
                Vec::leak(sofie_engine::compile_template("built", "").unwrap()),
            ),
        );
        let html = |template: &Template| {
            template
                .parsed_from_disk()
                .unwrap()
                .render(&json!({"name": "Ada"}))
                .unwrap()
        };
        assert_eq!(html(&template), "<p>old Ada</p>");

        std::fs::write(path, "<p>new {data.name}</p>").unwrap();

        assert_eq!(html(&template), "<p>new Ada</p>");
        std::fs::remove_file(path).unwrap();
    }
}
