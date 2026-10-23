use serde_json::{Map, Value};
use sofie_core::{Error, Result};

use crate::urlencoded::{bad_form, decode};

/// Forms nest one level; the bound keeps a hostile name from building a value too deep to drop.
const MAX_DEPTH: usize = 8;

/// A repeated name keeps its last value, as a checkbox after its hidden `false` needs.
pub(crate) fn parse(body: &[u8]) -> Result<Map<String, Value>> {
    let mut object = Map::new();
    for pair in body.split(|byte| *byte == b'&') {
        let (name, value) = match pair.iter().position(|byte| *byte == b'=') {
            Some(index) => (&pair[..index], &pair[index + 1..]),
            None => (pair, &[][..]),
        };
        let value = decode(value)?;
        if value.is_empty() {
            continue;
        }
        insert(&mut object, &decode(name)?, value)?;
    }
    Ok(object)
}

pub(crate) fn insert(object: &mut Map<String, Value>, name: &str, value: String) -> Result<()> {
    let segments: Vec<&str> = name.split('.').collect();
    if segments.len() > MAX_DEPTH
        || !segments
            .iter()
            .all(|segment| sofie_core::is_field_name(segment))
    {
        return Err(bad_form(
            "a field name is not letters, digits and `_`, nested with `.`",
        ));
    }
    let Some((last, parents)) = segments.split_last() else {
        return Err(bad_form("a field has no name"));
    };
    let mut current = object;
    for parent in parents {
        let slot = current
            .entry(*parent)
            .or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(nested) = slot else {
            return Err(value_and_group());
        };
        current = nested;
    }
    if current.get(*last).is_some_and(Value::is_object) {
        return Err(value_and_group());
    }
    current.insert((*last).to_owned(), Value::String(value));
    Ok(())
}

fn value_and_group() -> Error {
    bad_form("a field is posted both as a value and as a group of fields")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parsed(body: &str) -> Value {
        parse(body.as_bytes()).map_or_else(|error| panic!("{body}: {error}"), Value::Object)
    }

    fn refused(body: &[u8]) -> String {
        match parse(body) {
            Err(Error::BadRequest(message)) => message,
            other => panic!("{body:?} was not refused: {other:?}"),
        }
    }

    #[test]
    fn decodes_names_and_values() {
        assert_eq!(
            parsed("email=ada%40example.com&note=two+words%2B%26&name=J%C3%BCrgen"),
            json!({ "email": "ada@example.com", "note": "two words+&", "name": "Jürgen" })
        );
    }

    #[test]
    fn nests_dotted_names_and_keeps_the_last_of_a_repeated_name() {
        assert_eq!(
            parsed("total.amount=1&total.currency=EUR&total.amount=12.50&gift=false&gift=true"),
            json!({ "total": { "amount": "12.50", "currency": "EUR" }, "gift": "true" })
        );
    }

    #[test]
    fn drops_empty_values_and_empty_pairs() {
        assert_eq!(
            parsed("&email=&note&total.amount=&total.currency=EUR&&"),
            json!({ "total": { "currency": "EUR" } })
        );
        assert_eq!(parsed(""), json!({}));
    }

    #[test]
    fn refuses_names_that_are_not_plain_fields() {
        for body in [
            "a.=1",
            ".a=1",
            "a..b=1",
            "a-b=1",
            "a%20b=1",
            "=1",
            "a[]=1",
            "a.b.c.d.e.f.g.h.i=1",
        ] {
            assert!(refused(body.as_bytes()).contains("field name"), "{body}");
        }
        assert_eq!(
            parsed("a.b.c.d.e.f.g.h=1")["a"]["b"]["c"]["d"]["e"]["f"]["g"]["h"],
            "1"
        );
    }

    #[test]
    fn refuses_a_name_that_is_both_a_value_and_a_group() {
        for body in ["a=1&a.b=2", "a.b=2&a=1", "a.b=1&a.b.c=2"] {
            assert!(refused(body.as_bytes()).contains("both"), "{body}");
        }
        assert_eq!(parsed("a=&a.b=2"), json!({ "a": { "b": "2" } }));
    }

    #[test]
    fn refuses_broken_escapes_and_bytes_that_are_not_utf8() {
        for body in ["a=%", "a=%4", "a=%zz", "a=%+1", "%=1"] {
            assert!(refused(body.as_bytes()).contains('%'), "{body}");
        }
        assert!(refused(b"a=%C3%28").contains("UTF-8"));
        assert!(refused(b"a=\xff").contains("UTF-8"));
    }

    /// xorshift: deterministic, and enough to wander through odd inputs.
    fn noise(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn any_bytes_are_read_or_refused_without_panicking() {
        const ALPHABET: &[u8] = b"ab_.=&%+2Fz\xc3\xa9\xff ";
        let mut seed = 0x2545_f491_4f6c_dd1d;
        for _ in 0..20_000 {
            let length = usize::try_from(noise(&mut seed) % 24).unwrap();
            let body: Vec<u8> = (0..length)
                .map(|_| {
                    let index = usize::try_from(noise(&mut seed)).unwrap() % ALPHABET.len();
                    ALPHABET[index]
                })
                .collect();
            if let Err(error) = parse(&body) {
                assert!(matches!(error, Error::BadRequest(_)), "{body:?}");
            }
        }
    }

    #[test]
    fn a_long_hostile_body_is_refused_quickly() {
        let deep = format!("{}a=1", "a.".repeat(100_000));
        assert!(refused(deep.as_bytes()).contains("field name"));
        let many = "a=1&".repeat(100_000);
        assert_eq!(parsed(&many), json!({ "a": "1" }));
    }
}
