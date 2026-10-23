use sofie_core::encoding::percent;

use crate::context::parameter;
use crate::urlencoded::is_path_byte;

#[derive(Debug, PartialEq, Eq)]
enum Piece<'p> {
    Literal(&'p str),
    Parameter(&'p str),
    CatchAll(&'p str),
}

pub(crate) fn parameter_names(pattern: &str) -> Vec<&str> {
    pieces(pattern)
        .into_iter()
        .filter_map(|piece| match piece {
            Piece::Parameter(name) | Piece::CatchAll(name) => Some(name),
            Piece::Literal(_) => None,
        })
        .collect()
}

/// Matched values are decoded, so each is encoded again.
pub(crate) fn fill(pattern: &str, parameters: &[(String, String)]) -> Option<String> {
    let mut path = String::with_capacity(pattern.len());
    for piece in pieces(pattern) {
        match piece {
            Piece::Literal(text) => path.push_str(text),
            Piece::Parameter(name) => {
                percent::push_encoded(
                    &mut path,
                    parameter(parameters, name)?,
                    percent::is_unreserved,
                );
            }
            Piece::CatchAll(name) => {
                percent::push_encoded(&mut path, parameter(parameters, name)?, is_path_byte);
            }
        }
    }
    Some(path)
}

fn pieces(pattern: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut rest = pattern;
    while let Some((before, after)) = rest.split_once('{')
        && let Some((name, after)) = after.split_once('}')
    {
        pieces.push(Piece::Literal(before));
        pieces.push(match name.strip_prefix('*') {
            Some(name) => Piece::CatchAll(name),
            None => Piece::Parameter(name),
        });
        rest = after;
    }
    pieces.push(Piece::Literal(rest));
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matched(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn names_every_parameter() {
        assert_eq!(parameter_names("/orders"), Vec::<&str>::new());
        assert_eq!(
            parameter_names("/shops/{shop}/orders/{order}/{*rest}"),
            ["shop", "order", "rest"]
        );
    }

    #[test]
    fn fills_parameters_from_the_matched_values() {
        let values = matched(&[("order", "0190a7d2"), ("shop", "s 1/2"), ("rest", "a b/c")]);
        assert_eq!(fill("/orders", &values).as_deref(), Some("/orders"));
        assert_eq!(
            fill("/shops/{shop}/orders/{order}", &values).as_deref(),
            Some("/shops/s%201%2F2/orders/0190a7d2")
        );
        assert_eq!(
            fill("/files/{*rest}", &values).as_deref(),
            Some("/files/a%20b/c")
        );
        assert_eq!(fill("/members/{member}", &values), None);
    }
}
