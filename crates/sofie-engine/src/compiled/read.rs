use super::{FORMAT, MAGIC, tag};
use crate::catalog::Kind;
use crate::language::{Argument, Call};
use crate::message::{Message, Recorded};
use crate::syntax::{
    Attribute, Conditional, Expression, Loop, MAX_DEPTH, Op, Path, Position, Root, Written,
};

const REBUILD: &str = "rebuild the app so its build script compiles its screens again";

pub(crate) fn read(bytes: &[u8]) -> Result<Vec<Op<Written>>, String> {
    let rest = bytes
        .strip_prefix(MAGIC)
        .ok_or_else(|| format!("these are not a compiled template's bytes; {REBUILD}"))?;
    let (version, rest) = rest
        .split_first_chunk::<2>()
        .ok_or_else(|| damaged("the format version"))?;
    let version = u16::from_le_bytes(*version);
    if version != FORMAT {
        return Err(format!(
            "the screen was compiled in format {version}, and this sofie-engine reads format {FORMAT}; {REBUILD}"
        ));
    }
    let mut reader = Reader {
        bytes: rest,
        depth: 0,
        expression_depth: 0,
    };
    let ops = reader.ops()?;
    if !reader.bytes.is_empty() {
        return Err(damaged("bytes after the program"));
    }
    Ok(ops)
}

fn damaged(what: &str) -> String {
    format!("the compiled screen is damaged at {what}; {REBUILD}")
}

struct Reader<'b> {
    bytes: &'b [u8],
    depth: usize,
    expression_depth: usize,
}

impl Reader<'_> {
    fn ops(&mut self) -> Result<Vec<Op<Written>>, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(damaged("a nesting deeper than any template's"));
        }
        let count = self.number()?;
        let ops = (0..count).map(|_| self.op()).collect();
        self.depth -= 1;
        ops
    }

    fn op(&mut self) -> Result<Op<Written>, String> {
        Ok(match self.byte()? {
            tag::MARKUP => Op::Markup(self.text()?),
            tag::TEXT => Op::Text(self.expression()?),
            tag::URL => Op::Url(self.expression()?),
            tag::IDENTIFIED => Op::Identified {
                id: self.expression()?,
                body: self.ops()?,
            },
            tag::COMPONENT => Op::Component(self.component()?),
            tag::IF => Op::If(Conditional {
                condition: self.expression()?,
                then_branch: self.ops()?,
                else_branch: self.ops()?,
            }),
            tag::EACH => Op::Each(Loop {
                list: self.expression()?,
                has_index: self.byte()? != 0,
                body: self.ops()?,
            }),
            _ => return Err(damaged("an op")),
        })
    }

    fn component(&mut self) -> Result<Written, String> {
        let name = self.text()?;
        let kind = Kind::named(&name).ok_or_else(|| {
            format!(
                "the screen uses component `{name}`, which this app's sofie-engine features leave out; \
                 its catalog has {}",
                Kind::list()
            )
        })?;
        let position = self.position()?;
        let count = self.number()?;
        let attributes = (0..count)
            .map(|_| {
                Ok(Attribute {
                    name: self.text()?,
                    value: self.expression()?,
                    position: self.position()?,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Written {
            kind,
            attributes,
            children: self.ops()?,
            position,
        })
    }

    fn expression(&mut self) -> Result<Expression, String> {
        self.expression_depth += 1;
        if self.expression_depth > MAX_DEPTH {
            return Err(damaged("an expression nesting deeper than any template's"));
        }
        let expression = self.unnested_expression();
        self.expression_depth -= 1;
        expression
    }

    fn unnested_expression(&mut self) -> Result<Expression, String> {
        Ok(match self.byte()? {
            tag::PATH => {
                let root = match self.number()? {
                    0 => Root::Data,
                    slot => Root::Slot(slot - 1),
                };
                let count = self.number()?;
                let fields = (0..count).map(|_| self.text()).collect::<Result<_, _>>()?;
                Expression::Path(Path { root, fields })
            }
            tag::QUOTED => Expression::Text(self.text()?),
            tag::INTEGER => {
                let (number, rest) = self
                    .bytes
                    .split_first_chunk::<8>()
                    .ok_or_else(|| damaged("an integer"))?;
                self.bytes = rest;
                Expression::Integer(i64::from_le_bytes(*number))
            }
            tag::AUTHORED => Expression::Authored(self.text()?),
            tag::INTERPOLATED => {
                let count = self.number()?;
                let parts = (0..count)
                    .map(|_| self.expression())
                    .collect::<Result<_, _>>()?;
                Expression::Interpolated(parts)
            }
            tag::TRANSLATE => self.translation()?,
            call @ (tag::FORMAT_CALL | tag::LABEL) => self.call(call)?,
            tag::GATE => self.gate()?,
            _ => return Err(damaged("an expression")),
        })
    }

    fn translation(&mut self) -> Result<Expression, String> {
        let key = self.text()?;
        let count = self.number()?;
        let arguments = (0..count)
            .map(|_| {
                Ok(Argument {
                    name: self.text()?,
                    value: self.expression()?,
                })
            })
            .collect::<Result<_, String>>()?;
        let recorded = self.recorded()?;
        let call = Call::translate(key, arguments, self.position()?, recorded);
        Ok(Expression::Call(Box::new(call)))
    }

    fn recorded(&mut self) -> Result<Option<Recorded>, String> {
        let forms = self.byte()?;
        if forms == 0 {
            return Ok(None);
        }
        let other = self.message()?;
        let one = match forms {
            1 => None,
            2 => Some(self.message()?),
            _ => return Err(damaged("a recorded text")),
        };
        Ok(Some(Recorded { other, one }))
    }

    fn message(&mut self) -> Result<Message, String> {
        Message::parse(&self.text()?).map_err(|_| damaged("a recorded text"))
    }

    #[cfg(feature = "i18n")]
    fn call(&mut self, call: u8) -> Result<Expression, String> {
        use crate::translations::Format;
        let call = match call {
            tag::FORMAT_CALL => Call::Format {
                format: match self.byte()? {
                    0 => Format::Number,
                    1 => Format::Money,
                    tag::DATE_FORMAT => Format::Date,
                    _ => return Err(damaged("a format")),
                },
                value: self.expression()?,
                position: self.position()?,
            },
            _ => Call::Label {
                key: self.text()?,
                humanized: self.text()?,
                position: self.position()?,
            },
        };
        Ok(Expression::Call(Box::new(call)))
    }

    #[cfg(not(feature = "i18n"))]
    fn call(&mut self, call: u8) -> Result<Expression, String> {
        if call != tag::FORMAT_CALL || self.byte()? != tag::DATE_FORMAT {
            return Err(
                "the screen uses `number(…)`, `money(…)` or a label, which need sofie-engine's \
                 `i18n` feature"
                    .to_owned(),
            );
        }
        let value = self.expression()?;
        let call = Call::date(value, self.position()?);
        Ok(Expression::Call(Box::new(call)))
    }

    #[cfg(feature = "gates")]
    fn gate(&mut self) -> Result<Expression, String> {
        use crate::rendering::{Asks, Gate};
        let asks = match self.byte()? {
            0 => Asks::Permission,
            1 => Asks::Flag,
            2 => Asks::Entitlement,
            _ => return Err(damaged("a condition")),
        };
        Ok(Expression::Gate(Gate {
            asks,
            name: self.text()?,
        }))
    }

    #[cfg(not(feature = "gates"))]
    #[expect(
        clippy::unused_self,
        reason = "reads the condition under the `gates` feature"
    )]
    fn gate(&mut self) -> Result<Expression, String> {
        Err(
            "the screen uses `can(…)` or `enabled(…)`, which need sofie-engine's `gates` feature"
                .to_owned(),
        )
    }

    fn position(&mut self) -> Result<Position, String> {
        Ok(Position {
            line: self.number()?,
            column: self.number()?,
        })
    }

    fn text(&mut self) -> Result<String, String> {
        let length = self.number()?;
        if length > self.bytes.len() {
            return Err(damaged("a text"));
        }
        let (text, rest) = self.bytes.split_at(length);
        self.bytes = rest;
        String::from_utf8(text.to_vec()).map_err(|_| damaged("a text that is not UTF-8"))
    }

    fn byte(&mut self) -> Result<u8, String> {
        let (&byte, rest) = self.bytes.split_first().ok_or_else(|| damaged("its end"))?;
        self.bytes = rest;
        Ok(byte)
    }

    fn number(&mut self) -> Result<usize, String> {
        let mut number: usize = 0;
        for shift in (0..usize::BITS).step_by(7) {
            let byte = self.byte()?;
            number |= usize::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(number);
            }
        }
        Err(damaged("a number"))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{FORMAT, MAGIC, tag};
    use super::read;

    #[test]
    fn deeply_nested_expressions_are_refused_not_followed() {
        let mut bytes = MAGIC.to_vec();
        bytes.extend(FORMAT.to_le_bytes());
        bytes.extend([1, tag::TEXT]);
        for _ in 0..200_000 {
            bytes.extend([tag::INTERPOLATED, 1]);
        }
        bytes.extend([tag::AUTHORED, 0]);

        let error = read(&bytes).expect_err("the nesting is refused");

        assert!(error.contains("nesting"), "{error}");
    }
}
