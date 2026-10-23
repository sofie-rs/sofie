use std::fs;
use std::path::{Path, PathBuf};

use crate::explain::screen_file;
use crate::gen_command::{GenError, io_error};

pub(super) struct ScreenEdit {
    pub(super) key: String,
    pub(super) find: String,
    pub(super) replace: String,
}

pub(super) fn replace_once(root: &Path, edit: &ScreenEdit) -> Result<PathBuf, GenError> {
    if edit.find.is_empty() {
        return Err(GenError::Usage(
            "`find` is empty; give the exact text to replace".to_owned(),
        ));
    }
    let app = sofie_model::load(root).map_err(GenError::Model)?;
    let file = app
        .modules
        .iter()
        .find_map(|module| screen_file(module, &edit.key))
        .map(|(_, file)| file.clone())
        .ok_or_else(|| {
            GenError::Usage(format!(
                "no screen template `{}`; the `screens` tool lists them, and `scaffold_screen` \
                 writes a built-in screen to one",
                edit.key
            ))
        })?;
    let unreadable = |source| io_error(&file, source);
    let text = fs::read_to_string(&file).map_err(unreadable)?;
    // The first and last match differ also when two matches overlap.
    match (text.find(&edit.find), text.rfind(&edit.find)) {
        (None, _) => Err(GenError::Usage(format!(
            "`find` is not in {}; copy it exactly, whitespace included, from the markup \
             `explain {}` shows",
            file.display(),
            edit.key
        ))),
        (first, last) if first != last => Err(GenError::Usage(format!(
            "`find` occurs more than once in {}; include more of the text around it",
            file.display()
        ))),
        _ => fs::write(&file, text.replacen(&edit.find, &edit.replace, 1))
            .map(|()| file.clone())
            .map_err(unreadable),
    }
}
