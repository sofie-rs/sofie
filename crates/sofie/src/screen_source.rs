use sofie_core::{Error, Result};
use sofie_http::App;

pub(crate) fn print(key: &str, app: &App) -> Result<()> {
    let source = sofie_http::built_in_screen_source(app, key).ok_or_else(|| {
        Error::internal_message(format!("`{key}` names no built-in screen of this app"))
    })?;
    print!("{source}");
    Ok(())
}
