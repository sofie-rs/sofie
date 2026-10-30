//! What a client area's screen is given beside its data, answered for one request: the gates
//! its template asks, the messages it shows in the request's language, the language's formats,
//! its forms as the input types describe them, and the enums it shows.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use sofie_core::unpoisoned;
use sofie_core::value::serde::ser::{Serialize, SerializeMap, Serializer};

use crate::catalog::{FormPart, constant_label};
use crate::language::{Language, Texts};
use crate::message::Recorded;
use crate::syntax::{Expression, Op};
use crate::walk::{Visit, walk};

const ENGINE_KEYS: [&str; 4] = [
    "table.empty",
    "table.filter",
    "table.previous",
    "table.next",
];

/// What a client area's screen is given beside its data, for no request.
#[must_use]
pub fn plain_client_view(template: &crate::Template) -> Value {
    client_parts(template).view(Language::plain()).to_value()
}

pub(crate) fn client_parts(template: &crate::Template) -> &ClientParts {
    template
        .precomputed
        .client_parts
        .get_or_init(|| ClientParts::collect(&template.ops))
}

/// What a template's view is made of, collected once so a request only fills in its language
/// and viewer.
#[derive(Debug)]
pub(crate) struct ClientParts {
    #[cfg(feature = "gates")]
    gates: Vec<crate::rendering::Gate>,
    keys: BTreeSet<String>,
    counted: BTreeSet<String>,
    /// Each `t(…)` key's text compiled into the screen, written where the language has none.
    recorded: BTreeMap<String, Recorded>,
    forms: Vec<ClientForm>,
    enums: BTreeSet<String>,
    /// Where a table's rows lie in the data, which may hold a listing's refused filters.
    #[cfg(all(feature = "listing", any(feature = "gates", feature = "i18n")))]
    tables: Vec<Vec<String>>,
    /// One per language asked of the newest translations, so at most as many as the app
    /// declares.
    in_languages: Mutex<Vec<(Texts, LanguageView)>>,
}

type LanguageView = Arc<Map<String, Value>>;

#[derive(Debug)]
struct ClientForm {
    id: String,
    multipart: bool,
    parts: Vec<ClientFormPart>,
}

#[derive(Debug)]
enum ClientFormPart {
    Field {
        description: Value,
        label: Expression,
    },
    Submit,
}

impl ClientParts {
    pub(crate) fn collect(ops: &[Op]) -> Self {
        let mut found = Found::default();
        walk(ops, &mut found);
        Self {
            #[cfg(feature = "gates")]
            gates: found.gates.into_iter().cloned().collect(),
            keys: owned(found.keys),
            counted: owned(found.counted),
            recorded: found.recorded,
            forms: merged_forms(found.forms),
            enums: owned(found.enums),
            #[cfg(all(feature = "listing", any(feature = "gates", feature = "i18n")))]
            tables: found.tables,
            in_languages: Mutex::default(),
        }
    }

    pub(crate) fn view(&self, language: Language<'_>) -> ClientView {
        ClientView {
            in_language: self.in_language(language),
            #[cfg(feature = "gates")]
            gates: self.gates(language),
        }
    }

    fn in_language(&self, language: Language<'_>) -> LanguageView {
        let texts = language.texts();
        let mut in_languages = unpoisoned::lock(&self.in_languages);
        if let Some((_, view)) = in_languages.iter().find(|(cached, _)| *cached == texts) {
            return Arc::clone(view);
        }
        let mut view = Map::new();
        view.insert("messages".to_owned(), self.messages(language));
        view.insert("conventions".to_owned(), conventions(language));
        view.insert("forms".to_owned(), self.forms(language));
        view.insert("enums".to_owned(), self.enums(language));
        let view = Arc::new(view);
        in_languages.retain(|(cached, _)| cached.is_of_same_translations(texts));
        in_languages.push((texts, Arc::clone(&view)));
        view
    }
}

/// Serialized from the language's cached view, which is shared rather than copied per request.
#[derive(Debug)]
pub struct ClientView {
    in_language: LanguageView,
    #[cfg(feature = "gates")]
    gates: Value,
}

impl ClientView {
    #[must_use]
    pub fn to_value(&self) -> Value {
        let view = Map::clone(&self.in_language);
        #[cfg(feature = "gates")]
        let view = {
            let mut view = view;
            view.insert("gates".to_owned(), self.gates.clone());
            view
        };
        Value::Object(view)
    }
}

impl Serialize for ClientView {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        for (name, value) in self.in_language.iter() {
            map.serialize_entry(name, value)?;
        }
        #[cfg(feature = "gates")]
        map.serialize_entry("gates", &self.gates)?;
        map.end()
    }
}

fn owned(names: BTreeSet<&str>) -> BTreeSet<String> {
    names.into_iter().map(str::to_owned).collect()
}

/// Forms in an `{#if}`'s branches may share an id, by which alone the client looks one up, so
/// they are described as one.
fn merged_forms(forms: Vec<&crate::catalog::Form>) -> Vec<ClientForm> {
    let mut merged: Vec<ClientForm> = Vec::new();
    for form in forms.into_iter().map(ClientForm::of) {
        match merged.iter_mut().find(|known| known.id == form.id) {
            Some(known) => known.merge(form),
            None => merged.push(form),
        }
    }
    merged
}

impl ClientForm {
    fn merge(&mut self, other: Self) {
        self.multipart |= other.multipart;
        let submits = self.submit_count();
        let mut after_fields = self
            .parts
            .iter()
            .rposition(|part| matches!(part, ClientFormPart::Field { .. }))
            .map_or(0, |index| index + 1);
        let mut other_submits = 0;
        for part in other.parts {
            match part {
                ClientFormPart::Submit => other_submits += 1,
                ClientFormPart::Field {
                    ref description, ..
                } if self.has_field(description) => {}
                field @ ClientFormPart::Field { .. } => {
                    self.parts.insert(after_fields, field);
                    after_fields += 1;
                }
            }
        }
        self.parts
            .extend((submits..other_submits).map(|_| ClientFormPart::Submit));
    }

    fn submit_count(&self) -> usize {
        self.parts
            .iter()
            .filter(|part| matches!(part, ClientFormPart::Submit))
            .count()
    }

    fn has_field(&self, description: &Value) -> bool {
        self.parts.iter().any(|part| {
            matches!(part, ClientFormPart::Field { description: known, .. } if known["name"] == description["name"])
        })
    }

    fn of(form: &crate::catalog::Form) -> Self {
        let parts = form
            .parts
            .iter()
            .map(|part| match part {
                FormPart::Field(field) => ClientFormPart::Field {
                    description: field.client_description(),
                    label: field.label.clone(),
                },
                FormPart::Submit(_) => ClientFormPart::Submit,
            })
            .collect();
        let multipart = form
            .parts
            .iter()
            .any(|part| matches!(part, FormPart::Field(field) if field.is_file()));
        Self {
            id: form.id.clone(),
            multipart,
            parts,
        }
    }

    fn view(&self, language: Language<'_>) -> Value {
        let mut submits = 0;
        let parts: Vec<Value> = self
            .parts
            .iter()
            .map(|part| match part {
                ClientFormPart::Field { description, label } => {
                    let mut field = description.clone();
                    field["label"] = json!(constant_label(label, language));
                    json!({ "field": field })
                }
                ClientFormPart::Submit => {
                    submits += 1;
                    json!({ "submit": submits - 1 })
                }
            })
            .collect();
        json!({ "multipart": self.multipart, "parts": parts })
    }
}

#[derive(Default)]
struct Found<'t> {
    #[cfg(feature = "gates")]
    gates: Vec<&'t crate::rendering::Gate>,
    keys: BTreeSet<&'t str>,
    counted: BTreeSet<&'t str>,
    recorded: BTreeMap<String, Recorded>,
    forms: Vec<&'t crate::catalog::Form>,
    enums: BTreeSet<&'t str>,
    #[cfg(all(feature = "listing", any(feature = "gates", feature = "i18n")))]
    tables: Vec<Vec<String>>,
}

impl<'t> Visit<'t> for Found<'t> {
    /// A table inside a loop reads its rows from an item, which is not followed.
    #[cfg(all(feature = "listing", any(feature = "gates", feature = "i18n")))]
    fn columns(&mut self, rows: &'t Expression, _columns: &'t [String]) {
        if let Expression::Path(path) = rows
            && path.root == crate::syntax::Root::Data
        {
            self.tables.push(path.fields.clone());
        }
    }

    fn form(&mut self, form: &'t crate::catalog::Form) {
        self.forms.push(form);
    }

    fn enumeration(&mut self, name: &'t str) {
        self.enums.insert(name);
    }

    fn expression(&mut self, expression: &'t Expression) {
        match expression {
            Expression::Interpolated(parts) => {
                for part in parts {
                    self.expression(part);
                }
            }
            Expression::Call(call) => self.call(call),
            #[cfg(feature = "gates")]
            Expression::Gate(gate) => self.gates.push(gate),
            Expression::Path(_)
            | Expression::Text(_)
            | Expression::Integer(_)
            | Expression::Authored(_) => {}
        }
    }
}

#[cfg(feature = "i18n")]
impl<'t> Found<'t> {
    fn call(&mut self, call: &'t crate::translations::Call) {
        use crate::translations::Call;
        match call {
            Call::Translate {
                key,
                arguments,
                recorded,
                ..
            } => self.translation(key, arguments, recorded.as_ref()),
            Call::Format { value, .. } => self.expression(value),
            Call::Label { key, .. } => {
                self.keys.insert(key);
            }
        }
    }
}

#[cfg(not(feature = "i18n"))]
impl<'t> Found<'t> {
    fn call(&mut self, call: &'t crate::language::Call) {
        use crate::language::Call;
        match call {
            Call::Translate {
                key,
                arguments,
                recorded,
                ..
            } => self.translation(key, arguments, recorded.as_ref()),
            Call::Date { value, .. } => self.expression(value),
        }
    }
}

impl<'t> Found<'t> {
    fn translation(
        &mut self,
        key: &'t str,
        arguments: &'t [crate::language::Argument],
        recorded: Option<&Recorded>,
    ) {
        self.keys.insert(key);
        if arguments.iter().any(|argument| argument.name == "count") {
            self.counted.insert(key);
        }
        if let Some(recorded) = recorded {
            self.recorded.insert(key.to_owned(), recorded.clone());
        }
        for argument in arguments {
            self.expression(&argument.value);
        }
    }
}

impl ClientParts {
    #[cfg(feature = "gates")]
    fn gates(&self, language: Language<'_>) -> Value {
        let mut gates = json!({ "permission": {}, "flag": {}, "entitlement": {} });
        for gate in &self.gates {
            gates[gate.asks.name()][gate.name.as_str()] = Value::Bool(gate.holds(language.gates()));
        }
        gates
    }

    /// Only what the language says otherwise than the template's own fallback.
    #[cfg(feature = "i18n")]
    fn messages(&self, language: Language<'_>) -> Value {
        let plain = self
            .keys
            .iter()
            .map(String::as_str)
            .chain(ENGINE_KEYS)
            .map(str::to_owned);
        let others = self.counted.iter().map(|key| format!("{key}.other"));
        let ones = self.counted.iter().filter_map(|key| {
            let one = format!("{key}.one");
            Some((language.own_message(&one)?, one))
        });
        let mut messages: Map<String, Value> = plain
            .chain(others)
            .filter_map(|key| Some((language.message(&key)?, key)))
            .chain(ones)
            .map(|(message, key)| (key, Value::String(message.written())))
            .collect();
        for (key, recorded) in &self.recorded {
            if !messages.contains_key(key) && !messages.contains_key(&format!("{key}.other")) {
                self.write_recorded(&mut messages, key, recorded);
            }
        }
        Value::Object(messages)
    }

    #[cfg(not(feature = "i18n"))]
    fn messages(&self, _language: Language<'_>) -> Value {
        let _ = (&self.keys, ENGINE_KEYS);
        let mut messages = Map::new();
        for (key, recorded) in &self.recorded {
            self.write_recorded(&mut messages, key, recorded);
        }
        Value::Object(messages)
    }

    /// A listing's refused filters are sent with their keys; each message is written in the
    /// viewer's language as the HTML table writes it.
    #[cfg(all(feature = "listing", any(feature = "gates", feature = "i18n")))]
    pub(crate) fn translate_listing_errors(&self, data: &mut Value, language: Language<'_>) {
        for path in &self.tables {
            let errors = path
                .iter()
                .try_fold(&mut *data, |value, field| value.get_mut(field.as_str()))
                .and_then(|listed| listed.get_mut(sofie_core::listing::ERRORS))
                .and_then(Value::as_object_mut);
            for refusal in errors.into_iter().flat_map(|errors| errors.values_mut()) {
                translate_refusal(refusal, language);
            }
        }
    }

    /// Under the keys the client looks a call up by, as the language's own texts are.
    fn write_recorded(&self, messages: &mut Map<String, Value>, key: &str, recorded: &Recorded) {
        if !self.counted.contains(key) {
            messages.insert(key.to_owned(), Value::String(recorded.other.written()));
            return;
        }
        let other = Value::String(recorded.other.written());
        messages.insert(format!("{key}.other"), other);
        if let Some(one) = &recorded.one {
            messages.insert(format!("{key}.one"), Value::String(one.written()));
        }
    }

    fn forms(&self, language: Language<'_>) -> Value {
        let forms: Map<String, Value> = self
            .forms
            .iter()
            .map(|form| (form.id.clone(), form.view(language)))
            .collect();
        Value::Object(forms)
    }

    #[cfg(feature = "i18n")]
    fn enums(&self, language: Language<'_>) -> Value {
        let enums: Map<String, Value> = self
            .enums
            .iter()
            .map(|name| {
                let variants: Vec<Value> = language
                    .enumeration(name)
                    .unwrap_or_default()
                    .iter()
                    .map(|variant| {
                        json!({
                            "name": variant.name,
                            "label": language.text(&variant.key, &variant.humanized),
                            "class": variant.tone.class(),
                        })
                    })
                    .collect();
                (name.clone(), Value::Array(variants))
            })
            .collect();
        Value::Object(enums)
    }

    #[cfg(not(feature = "i18n"))]
    fn enums(&self, _language: Language<'_>) -> Value {
        let _ = &self.enums;
        Value::Object(Map::new())
    }
}

#[cfg(all(feature = "listing", any(feature = "gates", feature = "i18n")))]
fn translate_refusal(refusal: &mut Value, language: Language<'_>) {
    use sofie_core::listing::{REFUSAL_ARGUMENTS, REFUSAL_KEY, REFUSAL_MESSAGE};
    let Some(record) = refusal.as_object_mut() else {
        return;
    };
    let translated = {
        let text = |name: &str| record.get(name).and_then(Value::as_str);
        let (Some(key), Some(message)) = (text(REFUSAL_KEY), text(REFUSAL_MESSAGE)) else {
            return;
        };
        let arguments: Vec<(&str, String)> = record
            .get(REFUSAL_ARGUMENTS)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(name, value)| Some((name.as_str(), value.as_str()?.to_owned())))
            .collect();
        language.field_error(key, &arguments, message)
    };
    record.insert(REFUSAL_MESSAGE.to_owned(), Value::String(translated));
}

#[cfg(feature = "i18n")]
fn conventions(language: Language<'_>) -> Value {
    let mut described = language.conventions().describe();
    described["plural"] = Value::from(language.plural_rule().name());
    described["zone"] = Value::from(sofie_core::DisplayZone::current().name());
    described
}

/// One language without translations writes amounts as `1234.50 EUR` and days as ISO dates;
/// instants are shown in the display zone either way.
#[cfg(not(feature = "i18n"))]
fn conventions(_language: Language<'_>) -> Value {
    json!({
        "decimal": ".",
        "group": null,
        "money": "code-after",
        "days": "iso",
        "zone": sofie_core::DisplayZone::current().name(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use sofie_values::{Control, FormField, Input};

    use crate::Template;

    #[test]
    fn a_view_serializes_as_its_value_does() {
        let template = Template::parse(
            "screen",
            "{t(\"cart.items\", count = data.n)}<Form id=\"f\" post=\"/f\">\
             <Field name=\"note\" /></Form>",
        )
        .unwrap();
        let view = super::client_parts(&template).view(crate::language::Language::plain());

        assert_eq!(serde_json::to_value(&view).unwrap(), view.to_value());
    }

    #[test]
    fn forms_sharing_an_id_in_two_branches_are_described_as_one() {
        let template = Template::parse(
            "screen",
            "{#if data.a}<Form id=\"f\" post=\"/f\"><Field name=\"a\" /><Submit label=\"Go\" />\
             </Form>{:else}<Form id=\"f\" post=\"/f\"><Field name=\"b\" /><Field name=\"a\" />\
             <Submit label=\"Go\" /><Submit label=\"Later\" /></Form>{/if}",
        )
        .unwrap();

        let view = super::plain_client_view(&template);
        let parts = view["forms"]["f"]["parts"].as_array().unwrap();
        let shapes: Vec<Value> = parts
            .iter()
            .map(|part| part["field"]["name"].clone())
            .collect();

        assert_eq!(shapes, [json!("a"), json!("b"), Value::Null, Value::Null]);
        assert_eq!(parts[3], json!({ "submit": 1 }));
    }

    #[test]
    fn a_view_names_the_forms_and_enums_a_screen_shows() {
        let template = Template::parse(
            "screen",
            "<Form id=\"f\" post=\"/f\"><Field name=\"note\" required /><Submit label=\"Go\" />\
             </Form>",
        )
        .unwrap();

        let view = super::plain_client_view(&template);

        assert_eq!(
            view["forms"]["f"],
            json!({
                "multipart": false,
                "parts": [
                    {
                        "field": {
                            "name": "note",
                            "label": "Note",
                            "control": "input",
                            "attributes": [["type", "text"]],
                            "options": [],
                            "parts": [],
                            "required": true,
                            "constraints": [],
                        },
                    },
                    { "submit": 0 },
                ],
            })
        );
        assert_eq!(view["conventions"]["decimal"], ".");
        assert_eq!(view["conventions"]["zone"], "UTC");
    }

    #[test]
    fn a_view_follows_a_form_described_after_it_was_first_asked() {
        const PASSWORD: &[FormField] = &[FormField {
            name: "password",
            input: Input::new(Control::Password),
            required: true,
            constraints: &[],
        }];
        let mut template = Template::parse(
            "screen",
            "<Form id=\"f\" post=\"/f\"><Field name=\"password\" /></Form>",
        )
        .unwrap();
        let field = |template: &Template| {
            super::plain_client_view(template)["forms"]["f"]["parts"][0]["field"].clone()
        };
        assert_eq!(field(&template)["control"], "input");

        template.describe_form("f", PASSWORD).unwrap();

        assert_eq!(field(&template)["control"], "password");
        assert_eq!(field(&template)["label"], "Password");
    }
}
