use std::path::Path;

use crate::load::{files_with_extension, subdirectories};
use crate::naming::file_stem;
use crate::{MailTemplate, ModelError};

pub(crate) fn read_mails(module: &Path, crate_name: &str) -> Result<Vec<MailTemplate>, ModelError> {
    let folder = module.join("src").join("mails");
    let mut mails = keyed(&folder, crate_name)?;
    for replaced in subdirectories(&folder)? {
        mails.extend(keyed(&replaced, &file_stem(&replaced))?);
    }
    Ok(mails)
}

fn keyed(folder: &Path, owner: &str) -> Result<Vec<MailTemplate>, ModelError> {
    let files = files_with_extension(folder, "html")?;
    Ok(files
        .into_iter()
        .map(|file| MailTemplate {
            key: format!("{owner}/{}", file_stem(&file)),
            file,
        })
        .collect())
}
