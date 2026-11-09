use sofie_model::{App, Module};

use super::Edits;
use super::edits::module_file;
use crate::gen_command::GenError;
use crate::source_edits::read;

/// A notification kind other code sends with `Notify::send`, so it stays allowed as unused until then.
pub(super) fn new_topic(
    edits: &mut Edits,
    app: &App,
    module: &Module,
    name: &str,
) -> Result<(), GenError> {
    let file = module_file(&module.krate, "topics");
    let constant = name.to_ascii_uppercase();
    let topic = format!("{}.{name}", module.krate.crate_name);
    if file.is_file() {
        let text = read(&file)?;
        if text.contains(&format!("const {constant}:")) || text.contains(&format!("\"{topic}\"")) {
            return Err(GenError::Usage(format!(
                "{} declares the topic `{topic}` already",
                file.display()
            )));
        }
    }
    edits.enable_framework_feature(&module.krate, "notify")?;
    edits.append(
        &file,
        &[("use sofie::notify", "Topic")],
        &format!("pub const {constant}: Topic = Topic::new(\"{topic}\");\n"),
    )?;
    edits.declare(&module.krate, &["topics"])?;
    edits.allow_unused(&app.root, &module.krate.package, &constant, "topic")
}
