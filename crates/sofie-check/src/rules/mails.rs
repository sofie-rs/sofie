use std::fs;

use sofie_model::{MailTemplate, Module};

use super::{Finding, Rule};

/// The build compiles each mail template and stops at the first it refuses.
pub(super) fn check_mails(modules: &[Module]) -> Vec<Finding> {
    modules
        .iter()
        .flat_map(|module| &module.mails)
        .filter_map(check_mail)
        .collect()
}

fn check_mail(mail: &MailTemplate) -> Option<Finding> {
    let finding = |line: usize, message: String| Finding {
        rule: Rule::MailTemplate,
        file: mail.file.clone(),
        line,
        message,
        fix: "a mail binds what it is sent with under `data`, as `{data.service}`; fix the \
              template so the screen engine accepts it"
            .to_owned(),
    };
    let source = match fs::read_to_string(&mail.file) {
        Ok(source) => source,
        Err(error) => {
            return Some(finding(
                1,
                format!("the mail template cannot be read ({error}), so it was not checked"),
            ));
        }
    };
    let error = sofie_engine::compile_template(&mail.key, &source).err()?;
    Some(finding(
        error.line,
        format!(
            "mail `{}` does not compile at column {}: {}; the build would stop at it",
            mail.key, error.column, error.message
        ),
    ))
}
