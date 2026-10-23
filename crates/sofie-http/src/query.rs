use serde_json::Value;
use sofie_core::{Error, Result};
use sofie_db::{FieldNames, Input, Resolve};

use crate::{Area, RequestContext};

/// A GET's query string read as `Q`, an `Input` checked as a body is: a refused value is a 422
/// with its field errors, a key `Q` has no field for a 400 unless it is a tracking key. `Query`
/// alone reads keys by name.
pub struct Query<Q = QueryText>(pub Q);

/// The query string as it came, for a caller such as an OAuth provider sending keys of its own.
pub struct QueryText(Option<String>);

impl Query {
    /// The first value of `name`, percent-decoded; none when it is absent or not text.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<String> {
        self.0
            .0
            .as_deref()?
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == name)
            .and_then(|(_, value)| crate::urlencoded::decode(value.as_bytes()).ok())
    }
}

impl<A: Area, M> Resolve<RequestContext<A, M>> for Query {
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(Ok(Self(QueryText(context.query().map(str::to_owned)))))
    }
}

impl<A: Area, M, Q: Input + FieldNames + Send> Resolve<RequestContext<A, M>> for Query<Q> {
    fn resolve(context: &mut RequestContext<A, M>) -> impl Future<Output = Result<Self>> + Send {
        std::future::ready(typed(context.query().unwrap_or_default()).map(Self))
    }
}

/// What a link shared through ads or social sites carries; a link with one appended still opens.
pub(crate) fn is_tracking_key(key: &str) -> bool {
    key.starts_with("utm_") || matches!(key, "fbclid" | "gclid" | "msclkid")
}

fn typed<Q: Input + FieldNames>(query: &str) -> Result<Q> {
    let mut asked = crate::form::parse(query.as_bytes())?;
    asked.retain(|key, _| Q::FIELD_NAMES.contains(&key.as_str()) || !is_tracking_key(key));
    if let Some(unknown) = asked
        .keys()
        .find(|key| !Q::FIELD_NAMES.contains(&key.as_str()))
    {
        return Err(Error::BadRequest(format!(
            "the query has no parameter `{unknown}`"
        )));
    }
    Q::from_json(Value::Object(asked))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use sofie_core::{Error, FieldError, Result};
    use sofie_db::{FieldNames, Input, InputFields};

    use super::{Query, QueryText, typed};

    #[test]
    fn a_value_is_read_by_name_and_decoded() {
        let query = Query(QueryText(Some("state=a%2Bb&code=x+y&empty=".to_owned())));

        assert_eq!(query.get("state").as_deref(), Some("a+b"));
        assert_eq!(query.get("code").as_deref(), Some("x y"));
        assert_eq!(query.get("empty").as_deref(), Some(""));
        assert_eq!(query.get("missing"), None);
    }

    #[derive(Debug, PartialEq)]
    struct Around {
        near: String,
        radius: Option<u32>,
    }

    impl Input for Around {
        fn from_json(value: Value) -> Result<Self> {
            let mut fields = InputFields::new(value)?;
            let near = fields.take("near");
            let radius = fields.optional::<u32>("radius");
            let radius = fields.checked(radius, |radius| {
                radius
                    .is_some_and(|radius| radius > 50)
                    .then(|| FieldError::new("radius", "validation.max", "must be at most 50"))
            });
            fields.finish()?;
            Ok(Self {
                near: near?,
                radius: radius?,
            })
        }
    }

    impl FieldNames for Around {
        const FIELD_NAMES: &'static [&'static str] = &["near", "radius"];
    }

    #[test]
    fn a_typed_query_reads_its_fields_as_their_types() {
        let around: Around = typed("near=Berlin%20Mitte&radius=5").unwrap();

        assert_eq!(
            around,
            Around {
                near: "Berlin Mitte".to_owned(),
                radius: Some(5),
            }
        );
        let blank: Around = typed("near=Ulm&radius=").unwrap();
        assert_eq!(blank.radius, None);
    }

    #[test]
    fn a_refused_value_is_a_field_error_and_an_unknown_key_a_bad_request() {
        let refused = typed::<Around>("radius=80");
        let Err(Error::Invalid(errors)) = refused else {
            panic!("{:?}", refused.err());
        };
        let keys: Vec<(&str, &str)> = errors
            .iter()
            .map(|error| (error.field.as_str(), error.key.as_ref()))
            .collect();
        assert_eq!(
            keys,
            [
                ("near", "validation.required"),
                ("radius", "validation.max")
            ]
        );
        assert!(matches!(
            typed::<Around>("near=Ulm&page=2"),
            Err(Error::BadRequest(message)) if message.contains("`page`")
        ));
        assert!(matches!(typed::<()>("near=Ulm"), Err(Error::BadRequest(_))));
        assert!(typed::<()>("").is_ok());
    }

    #[test]
    fn a_tracking_key_appended_to_a_shared_link_is_left_unread() {
        let around: Around =
            typed("near=Ulm&utm_source=newsletter&utm_campaign=fall&fbclid=a&gclid=b&msclkid=c")
                .unwrap();

        assert_eq!(around.near, "Ulm");
        assert!(typed::<()>("utm_medium=email").is_ok());
        assert!(matches!(
            typed::<Around>("near=Ulm&utm=x"),
            Err(Error::BadRequest(message)) if message.contains("`utm`")
        ));
    }
}
