use sofie_model::Module;

use super::Edits;
use super::edits::refuse_function;
use crate::gen_command::GenError;

const TEMPLATE: &str = "<p>Write the mail here; it shows what its function puts in `data`.</p>\n";
const MAXIMUM_LIST: usize = 64;

/// `src/mails/<name>.html` and a function sending it from `src/mails.rs`; with `list`, as
/// bulk mail with a one-click unsubscribe link, which `sofie::mail::web::routes` answers.
pub(super) fn new_mail(
    edits: &mut Edits,
    module: &Module,
    name: &str,
    list: Option<&str>,
) -> Result<(), GenError> {
    let source = module.krate.path.join("src");
    let template = source.join("mails").join(format!("{name}.html"));
    if template.is_file() {
        return Err(GenError::Usage(format!(
            "{} exists already; choose another name",
            template.display()
        )));
    }
    let file = source.join("mails.rs");
    refuse_function(&file, name)?;
    if let Some(list) = list {
        checked_list(list)?;
    }
    edits.enable_framework_feature(
        &module.krate,
        if list.is_some() { "mail-web" } else { "mail" },
    )?;
    edits.create(&template, TEMPLATE)?;
    edits.append(
        &file,
        &[
            ("use sofie::db", "Queued"),
            ("use sofie::mail", "Address"),
            ("use sofie::mail", "Mail"),
            ("use sofie::mail", "Mailer"),
            ("use sofie::prelude", "*"),
        ],
        &sender(&module.krate.crate_name, name, list),
    )?;
    edits.declare(&module.krate, &["mails"])
}

fn sender(module: &str, name: &str, list: Option<&str>) -> String {
    let subject = sofie_engine::humanize(name);
    let bulk = list.map_or_else(String::new, |list| format!(".bulk(\"{list}\")?"));
    format!(
        "/// Queued with the handler's commit; fill `data` with what `src/mails/{name}.html` shows.\n\
         pub fn {name}(mailer: &Mailer, to: Address) -> Result<Queued> {{\n    \
         let body = mailer.body(\"{module}/{name}\", &sofie::serde_json::json!({{}}))?;\n    \
         mailer.send(Mail::new(to, \"{subject}\", body)?{bulk})\n}}\n"
    )
}

fn checked_list(list: &str) -> Result<(), GenError> {
    let named = !list.is_empty()
        && list.len() <= MAXIMUM_LIST
        && list
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if named {
        return Ok(());
    }
    Err(GenError::Usage(format!(
        "the list `{list}` is not 1 to 64 lowercase letters, digits and `-`"
    )))
}

#[cfg(test)]
mod tests {
    use super::sender;

    #[test]
    fn a_bulk_mail_sends_with_its_list() {
        let function = sender("shop", "weekly_news", Some("news"));

        assert!(
            function.contains("pub fn weekly_news(mailer: &Mailer, to: Address)"),
            "{function}"
        );
        assert!(
            function.contains("mailer.body(\"shop/weekly_news\""),
            "{function}"
        );
        assert!(
            function.contains("Mail::new(to, \"Weekly news\", body)?.bulk(\"news\")?"),
            "{function}"
        );
    }
}
