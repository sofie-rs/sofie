use sofie_core::{Error, Result};
use sofie_flags::{DeclaredSetting, Features, Flags, SettingValue as Held};
use sofie_http::{Area, RequestContext};

use crate::{Module, SettingValue};
use sofie_db::Resolve;

/// Changed at runtime on sofie-identity's settings screen; the wiring declares every module's.
pub fn declare_settings(flags: &Flags, modules: &[Module]) -> Result<()> {
    let declared = modules
        .iter()
        .flat_map(|module| {
            module.settings.iter().map(|setting| DeclaredSetting {
                name: format!("{}.{}", module.name, setting.name),
                default: held(setting.default),
            })
        })
        .collect();
    flags.declare_settings(declared)
}

fn held(value: SettingValue) -> Held {
    match value {
        SettingValue::Switch(on) => Held::Switch(on),
        SettingValue::Number(number) => Held::Number(number),
        SettingValue::Text(text) => Held::Text(text.to_owned()),
    }
}

/// Read from the flags' snapshot the request started with, so one request sees one value.
pub struct RuntimeSettings(Features);

impl From<Features> for RuntimeSettings {
    fn from(features: Features) -> Self {
        Self(features)
    }
}

impl<A: Area, M> Resolve<RequestContext<A, M>> for RuntimeSettings {
    async fn resolve(context: &mut RequestContext<A, M>) -> Result<Self> {
        Features::resolve(context).await.map(Self)
    }
}

impl RuntimeSettings {
    pub fn switch(&self, module: &Module, name: &str) -> Result<bool> {
        match self.held(module, name)? {
            Held::Switch(on) => Ok(on),
            _ => Err(another_kind(module, name, "switch")),
        }
    }

    pub fn number(&self, module: &Module, name: &str) -> Result<i64> {
        match self.held(module, name)? {
            Held::Number(number) => Ok(number),
            _ => Err(another_kind(module, name, "number")),
        }
    }

    pub fn text(&self, module: &Module, name: &str) -> Result<String> {
        match self.held(module, name)? {
            Held::Text(text) => Ok(text),
            _ => Err(another_kind(module, name, "text")),
        }
    }

    fn held(&self, module: &Module, name: &str) -> Result<Held> {
        let declared = module
            .settings
            .iter()
            .find(|setting| setting.name == name)
            .ok_or_else(|| {
                Error::internal_message(format!(
                    "module `{}` declares no setting `{name}` in its manifest",
                    module.name
                ))
            })?;
        Ok(self
            .0
            .setting(&format!("{}.{name}", module.name), held(declared.default)))
    }
}

fn another_kind(module: &Module, name: &str, asked: &str) -> Error {
    Error::internal_message(format!(
        "setting `{}.{name}` is read as a {asked}, but its declared default is of another kind",
        module.name
    ))
}
