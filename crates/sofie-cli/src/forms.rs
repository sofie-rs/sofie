use sofie_check::Screens;
use sofie_model::naming::{file_stem, screen_key};
use sofie_model::{App, AreaContribution, Module};

use crate::gen_command::{GenError, readable_screen};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DescribedForm {
    pub(crate) screen: String,
    pub(crate) form: String,
    pub(crate) input: String,
}

pub(crate) fn described_forms(
    app: &App,
    screens: &Screens,
) -> Result<Vec<DescribedForm>, GenError> {
    let mut forms = Vec::new();
    for module in &app.modules {
        for area in &module.areas {
            forms.extend(area_forms(app, screens, module, area)?);
        }
    }
    forms.sort_by(|left, right| (&left.screen, &left.form).cmp(&(&right.screen, &right.form)));
    Ok(forms)
}

fn area_forms(
    app: &App,
    screens: &Screens,
    module: &Module,
    area: &AreaContribution,
) -> Result<Vec<DescribedForm>, GenError> {
    let mut forms = Vec::new();
    for file in &area.screens {
        let screen = screen_key(&module.krate.crate_name, &area.area, &file_stem(file));
        let template = readable_screen(screens, file)?
            .template
            .as_ref()
            .map_err(|error| GenError::Screen {
                path: file.clone(),
                message: error.to_string(),
            })?;
        for post in template.form_posts() {
            let Some(input) = posted_input(app, &post.post) else {
                continue;
            };
            forms.push(DescribedForm {
                screen: screen.clone(),
                form: post.id,
                input,
            });
        }
    }
    Ok(forms)
}

fn posted_input(app: &App, post: &str) -> Option<String> {
    let site = app.posted_route(post)?;
    let code = &site.module.code;
    let input = code.input_of(code.function(&site.route.handler)?)?;
    Some(format!(
        "{}::{}",
        site.module.krate.crate_name,
        input.path.join("::")
    ))
}
