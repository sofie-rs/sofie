use sofie_model::App;

use super::json::Json;
use super::{
    Answer, Api, Endpoint, Property, Scalar, Schema, Shape, VALUE_SHAPES, ValueShape,
    value_properties,
};

const SCHEMAS: &str = "#/components/schemas/";
const COMMITTED: &str = "Committed";

#[must_use]
pub fn openapi(app: &App) -> String {
    let api = Api::of(app);
    let mut tags: Vec<&str> = api
        .endpoints
        .iter()
        .map(|endpoint| endpoint.area.as_str())
        .collect();
    tags.sort_unstable();
    tags.dedup();
    Json::object([
        ("openapi", Json::text("3.1.0")),
        (
            "info",
            Json::object([
                ("title", Json::text(&api.title)),
                ("version", Json::text("unversioned")),
            ]),
        ),
        (
            "tags",
            Json::Array(
                tags.iter()
                    .map(|tag| Json::object([("name", Json::text(tag))]))
                    .collect(),
            ),
        ),
        ("paths", paths(&api.endpoints)),
        ("components", Json::object([("schemas", schemas(&api))])),
    ])
    .render()
}

fn paths(endpoints: &[Endpoint]) -> Json {
    let mut paths: Vec<(String, Json)> = Vec::new();
    for endpoint in endpoints {
        let operation = (
            endpoint.method.name().to_ascii_lowercase(),
            operation(endpoint),
        );
        match paths.iter_mut().find(|(path, _)| *path == endpoint.path) {
            Some((_, Json::Object(operations))) => operations.push(operation),
            _ => paths.push((endpoint.path.clone(), Json::Object(vec![operation]))),
        }
    }
    Json::Object(paths)
}

fn operation(endpoint: &Endpoint) -> Json {
    let mut fields = vec![
        ("operationId".to_owned(), Json::text(&endpoint.name)),
        (
            "tags".to_owned(),
            Json::Array(vec![Json::text(&endpoint.area)]),
        ),
    ];
    if let Some(summary) = &endpoint.summary {
        fields.push(("summary".to_owned(), Json::text(summary)));
    }
    let parameters = parameters(endpoint);
    if !parameters.is_empty() {
        fields.push(("parameters".to_owned(), Json::Array(parameters)));
    }
    if let Some(input) = &endpoint.input {
        fields.push(("requestBody".to_owned(), request_body(input)));
    }
    fields.push(("responses".to_owned(), responses(endpoint)));
    Json::Object(fields)
}

fn parameters(endpoint: &Endpoint) -> Vec<Json> {
    let path = endpoint.parameters.iter().map(|name| {
        Json::object([
            ("name", Json::text(name)),
            ("in", Json::text("path")),
            ("required", Json::Boolean(true)),
            ("schema", Json::object([("type", Json::text("string"))])),
        ])
    });
    let query = endpoint
        .query
        .iter()
        .flat_map(|query| &query.properties)
        .map(|property| {
            let shape = match &property.shape {
                Shape::Optional(inner) => inner,
                shape => shape,
            };
            Json::object([
                ("name", Json::text(&property.name)),
                ("in", Json::text("query")),
                ("required", Json::Boolean(property.required)),
                ("schema", schema_of(shape)),
            ])
        });
    path.chain(query).collect()
}

fn request_body(input: &str) -> Json {
    Json::object([
        ("required", Json::Boolean(true)),
        (
            "content",
            Json::object([(
                "application/json",
                Json::object([("schema", reference(input))]),
            )]),
        ),
    ])
}

fn responses(endpoint: &Endpoint) -> Json {
    let mut responses = vec![match &endpoint.output {
        Answer::Empty => (
            "204".to_owned(),
            Json::object([("description", Json::text("Done"))]),
        ),
        Answer::Data(shape) => (
            "200".to_owned(),
            Json::object([
                ("description", Json::text("The handler's data")),
                (
                    "content",
                    Json::object([(
                        "application/json",
                        Json::object([("schema", schema_of(shape))]),
                    )]),
                ),
            ]),
        ),
    }];
    if endpoint.input.is_some() || endpoint.query.is_some() {
        responses.push((
            "422".to_owned(),
            Json::object([("description", Json::text("A field is invalid"))]),
        ));
    }
    Json::Object(responses)
}

fn schemas(api: &Api) -> Json {
    let mut schemas = vec![(
        COMMITTED.to_owned(),
        object_schema(&[
            ("id", Scalar::Uuid, true),
            ("version", Scalar::Integer, true),
        ]),
    )];
    schemas.extend(
        VALUE_SHAPES
            .iter()
            .filter(|(name, _)| api.mentions_value(name))
            .map(|(name, shape)| ((*name).to_owned(), object_schema(shape))),
    );
    schemas.extend(api.schemas.iter().map(|(name, schema)| {
        let json = match schema {
            Schema::Enum(names) => Json::object([
                ("type", Json::text("string")),
                (
                    "enum",
                    Json::Array(names.iter().map(|name| Json::text(name)).collect()),
                ),
            ]),
            Schema::Object(properties) => properties_schema(properties),
        };
        (name.clone(), json)
    }));
    Json::Object(schemas)
}

fn object_schema(shape: ValueShape) -> Json {
    properties_schema(&value_properties(shape))
}

fn properties_schema(properties: &[Property]) -> Json {
    let required: Vec<Json> = properties
        .iter()
        .filter(|property| property.required)
        .map(|property| Json::text(&property.name))
        .collect();
    Json::object([
        ("type", Json::text("object")),
        (
            "properties",
            Json::Object(
                properties
                    .iter()
                    .map(|property| (property.name.clone(), schema_of(&property.shape)))
                    .collect(),
            ),
        ),
        ("required", Json::Array(required)),
    ])
}

fn schema_of(shape: &Shape) -> Json {
    match shape {
        Shape::Scalar(scalar) => scalar_schema(*scalar),
        Shape::Value(name) => reference(name),
        Shape::Named(name) => reference(name),
        Shape::Committed => reference(COMMITTED),
        Shape::List(item) => {
            Json::object([("type", Json::text("array")), ("items", schema_of(item))])
        }
        Shape::Optional(inner) => any_of(inner, "null"),
        Shape::Tested(inner) => any_of(inner, "boolean"),
        Shape::Unknown(written) => Json::object([(
            "description",
            Json::Text(format!(
                "`{written}`, which `sofie openapi` cannot describe without compiling; \
                 answer a read model"
            )),
        )]),
    }
}

/// Not `oneOf`: a value may match both branches, as a boolean does a tested boolean.
fn any_of(inner: &Shape, kind: &str) -> Json {
    Json::object([(
        "anyOf",
        Json::Array(vec![
            schema_of(inner),
            Json::object([("type", Json::text(kind))]),
        ]),
    )])
}

fn scalar_schema(scalar: Scalar) -> Json {
    let kind = match scalar {
        Scalar::Integer => "integer",
        Scalar::Number => "number",
        Scalar::Boolean => "boolean",
        _ => "string",
    };
    let mut fields = vec![("type".to_owned(), Json::text(kind))];
    if let Some(format) = scalar.format() {
        fields.push(("format".to_owned(), Json::text(format)));
    }
    Json::Object(fields)
}

fn reference(name: &str) -> Json {
    Json::object([("$ref", Json::Text(format!("{SCHEMAS}{name}")))])
}

#[cfg(test)]
mod tests {
    use super::{openapi, schema_of};
    use crate::api::tests::shop;
    use crate::api::{Scalar, Shape};

    #[test]
    fn describes_a_screen_and_an_action_with_their_schemas() {
        let document = openapi(&shop());

        let expected = include_str!("../../tests/golden/openapi.json");
        assert_eq!(document, expected, "{document}");
    }

    #[test]
    fn a_tested_boolean_matches_either_branch() {
        let tested = Shape::Tested(Box::new(Shape::Scalar(Scalar::Boolean)));

        let schema = schema_of(&tested).render();

        assert!(schema.starts_with("{\n  \"anyOf\""), "{schema}");
    }

    #[test]
    fn a_null_optional_of_an_undescribed_type_matches_either_branch() {
        let optional = Shape::Optional(Box::new(Shape::Unknown("Opaque".to_owned())));

        let schema = schema_of(&optional).render();

        assert!(schema.starts_with("{\n  \"anyOf\""), "{schema}");
    }
}
