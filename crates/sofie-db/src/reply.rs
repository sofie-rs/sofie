use std::fmt;

use serde::Serialize;
use serde_json::Value;

use sofie_core::{Error, Result, Show, Shown};

pub enum Reply {
    Empty,
    Json(Value),
    Data(Box<dyn ReplyData>),
    List(Vec<Reply>),
}

pub trait ReplyData: Send + Sync {
    fn write_json(&self, out: &mut Vec<u8>) -> serde_json::Result<()>;

    fn to_json(&self) -> serde_json::Result<Value>;

    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// `None` renders a screen from the JSON it converts to.
    fn shown(&self) -> Option<Shown<'_>> {
        None
    }
}

impl<T: Serialize + Send + Sync> ReplyData for T {
    fn write_json(&self, out: &mut Vec<u8>) -> serde_json::Result<()> {
        serde_json::to_writer(out, self)
    }

    fn to_json(&self) -> serde_json::Result<Value> {
        serde_json::to_value(self)
    }
}

/// Data a screen reads in place, which `#[derive(ReadModel)]` replies with.
pub struct Showing<T>(pub T);

impl<T: Show + Serialize + Send + Sync> ReplyData for Showing<T> {
    fn write_json(&self, out: &mut Vec<u8>) -> serde_json::Result<()> {
        self.0.write_json(out)
    }

    fn to_json(&self) -> serde_json::Result<Value> {
        self.0.to_json()
    }

    fn shown(&self) -> Option<Shown<'_>> {
        Some(self.0.show())
    }
}

impl Show for Reply {
    fn show(&self) -> Shown<'_> {
        self.shown().unwrap_or(Shown::Null)
    }
}

impl Reply {
    #[must_use]
    pub fn data(data: impl Serialize + Send + Sync + 'static) -> Self {
        Self::Data(Box::new(data))
    }

    /// `None` when some data is only `Serialize`: the whole reply then renders from its JSON.
    #[must_use]
    pub fn shown(&self) -> Option<Shown<'_>> {
        match self {
            Self::Empty => Some(Shown::Null),
            Self::Json(value) => Some(value.show()),
            Self::Data(data) => data.shown(),
            Self::List(replies) => replies
                .iter()
                .all(|reply| reply.shown().is_some())
                .then_some(Shown::List(replies)),
        }
    }

    pub fn into_json(self) -> Result<Value> {
        match self {
            Self::Json(value) => Ok(value),
            other => other.to_json(),
        }
    }

    pub fn to_json(&self) -> Result<Value> {
        match self {
            Self::Empty => Ok(Value::Null),
            Self::Json(value) => Ok(value.clone()),
            Self::Data(data) => data
                .to_json()
                .map_err(|error| refusal(data.as_ref(), error)),
            Self::List(replies) => replies.iter().map(Self::to_json).collect(),
        }
    }

    pub fn write_json(&self, out: &mut Vec<u8>) -> Result<()> {
        match self {
            Self::Empty => out.extend_from_slice(b"null"),
            Self::Json(value) => serde_json::to_writer(out, value)
                .map_err(|error| Error::internal("serializing a reply", error))?,
            Self::Data(data) => data
                .write_json(out)
                .map_err(|error| refusal(data.as_ref(), error))?,
            Self::List(replies) => {
                out.push(b'[');
                for (index, reply) in replies.iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    reply.write_json(out)?;
                }
                out.push(b']');
            }
        }
        Ok(())
    }
}

fn refusal(data: &dyn ReplyData, error: serde_json::Error) -> Error {
    Error::internal(format!("serializing a `{}` reply", data.type_name()), error)
}

impl fmt::Debug for Reply {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("Empty"),
            Self::Json(value) => formatter.debug_tuple("Json").field(value).finish(),
            Self::Data(data) => formatter
                .debug_tuple("Data")
                .field(&data.type_name())
                .finish(),
            Self::List(replies) => formatter.debug_tuple("List").field(replies).finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;

    fn written(reply: &Reply) -> String {
        let mut out = Vec::new();
        reply.write_json(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn typed_data_is_written_as_the_value_it_converts_to() {
        let reply = Reply::List(vec![
            Reply::data(BTreeMap::from([("name", "a\"b"), ("total", "1")])),
            Reply::Empty,
            Reply::Json(json!({ "id": 2 })),
        ]);

        let text = written(&reply);
        assert_eq!(text, r#"[{"name":"a\"b","total":"1"},null,{"id":2}]"#);
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            reply.into_json().unwrap()
        );
    }

    #[test]
    fn data_json_cannot_hold_is_an_internal_error_naming_its_type() {
        let reply = Reply::data(BTreeMap::from([((1, 2), "pair")]));

        let error = reply.write_json(&mut Vec::new()).unwrap_err();
        assert!(matches!(error, Error::Internal { .. }));
        assert!(error.to_string().contains("BTreeMap"), "{error}");
        assert!(matches!(reply.to_json(), Err(Error::Internal { .. })));
    }
}
