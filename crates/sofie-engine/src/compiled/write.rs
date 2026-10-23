use super::{FORMAT, MAGIC, tag};
use crate::language::{Argument, Call};
use crate::message::Recorded;
use crate::syntax::{Expression, Op, Position, Root, Written};

pub(crate) fn write(ops: &[Op<Written>]) -> Vec<u8> {
    let mut writer = Writer(MAGIC.to_vec());
    writer.0.extend_from_slice(&FORMAT.to_le_bytes());
    writer.ops(ops);
    writer.0
}

struct Writer(Vec<u8>);

impl Writer {
    fn ops(&mut self, ops: &[Op<Written>]) {
        self.number(ops.len());
        for op in ops {
            self.op(op);
        }
    }

    fn op(&mut self, op: &Op<Written>) {
        match op {
            Op::Markup(markup) => {
                self.0.push(tag::MARKUP);
                self.text(markup);
            }
            Op::Text(text) => {
                self.0.push(tag::TEXT);
                self.expression(text);
            }
            Op::Url(url) => {
                self.0.push(tag::URL);
                self.expression(url);
            }
            Op::Identified { id, body } => {
                self.0.push(tag::IDENTIFIED);
                self.expression(id);
                self.ops(body);
            }
            Op::Component(written) => self.component(written),
            Op::If(conditional) => {
                self.0.push(tag::IF);
                self.expression(&conditional.condition);
                self.ops(&conditional.then_branch);
                self.ops(&conditional.else_branch);
            }
            Op::Each(each) => {
                self.0.push(tag::EACH);
                self.expression(&each.list);
                self.0.push(u8::from(each.has_index));
                self.ops(&each.body);
            }
        }
    }

    fn component(&mut self, written: &Written) {
        self.0.push(tag::COMPONENT);
        self.text(written.kind.name());
        self.position(written.position);
        self.number(written.attributes.len());
        for attribute in &written.attributes {
            self.text(&attribute.name);
            self.expression(&attribute.value);
            self.position(attribute.position);
        }
        self.ops(&written.children);
    }

    fn expression(&mut self, expression: &Expression) {
        match expression {
            Expression::Path(path) => {
                self.0.push(tag::PATH);
                self.number(match path.root {
                    Root::Data => 0,
                    Root::Slot(slot) => slot + 1,
                });
                self.number(path.fields.len());
                for field in &path.fields {
                    self.text(field);
                }
            }
            Expression::Text(text) => {
                self.0.push(tag::QUOTED);
                self.text(text);
            }
            Expression::Integer(number) => {
                self.0.push(tag::INTEGER);
                self.0.extend_from_slice(&number.to_le_bytes());
            }
            Expression::Authored(text) => {
                self.0.push(tag::AUTHORED);
                self.text(text);
            }
            Expression::Interpolated(parts) => {
                self.0.push(tag::INTERPOLATED);
                self.number(parts.len());
                for part in parts {
                    self.expression(part);
                }
            }
            Expression::Call(call) => self.call(call),
            #[cfg(feature = "gates")]
            Expression::Gate(gate) => {
                self.0.push(tag::GATE);
                self.0.push(match gate.asks {
                    crate::rendering::Asks::Permission => 0,
                    crate::rendering::Asks::Flag => 1,
                    crate::rendering::Asks::Entitlement => 2,
                });
                self.text(&gate.name);
            }
        }
    }

    #[cfg(feature = "i18n")]
    fn call(&mut self, call: &Call) {
        use crate::translations::Format;
        match call {
            Call::Translate {
                key,
                arguments,
                recorded,
                ..
            } => self.translation(key, arguments, recorded.as_ref()),
            Call::Format { format, value, .. } => {
                self.0.push(tag::FORMAT_CALL);
                self.0.push(match format {
                    Format::Number => 0,
                    Format::Money => 1,
                    Format::Date => tag::DATE_FORMAT,
                });
                self.expression(value);
            }
            Call::Label { key, humanized, .. } => {
                self.0.push(tag::LABEL);
                self.text(key);
                self.text(humanized);
            }
        }
        self.position(call.position());
    }

    #[cfg(not(feature = "i18n"))]
    fn call(&mut self, call: &Call) {
        match call {
            Call::Translate {
                key,
                arguments,
                recorded,
                ..
            } => self.translation(key, arguments, recorded.as_ref()),
            Call::Date { value, .. } => {
                self.0.push(tag::FORMAT_CALL);
                self.0.push(tag::DATE_FORMAT);
                self.expression(value);
            }
        }
        self.position(call.position());
    }

    fn translation(&mut self, key: &str, arguments: &[Argument], recorded: Option<&Recorded>) {
        self.0.push(tag::TRANSLATE);
        self.text(key);
        self.number(arguments.len());
        for argument in arguments {
            self.text(&argument.name);
            self.expression(&argument.value);
        }
        let Some(recorded) = recorded else {
            self.0.push(0);
            return;
        };
        self.0.push(if recorded.one.is_some() { 2 } else { 1 });
        self.text(&recorded.other.written());
        if let Some(one) = &recorded.one {
            self.text(&one.written());
        }
    }

    fn position(&mut self, position: Position) {
        self.number(position.line);
        self.number(position.column);
    }

    fn text(&mut self, text: &str) {
        self.number(text.len());
        self.0.extend_from_slice(text.as_bytes());
    }

    fn number(&mut self, number: usize) {
        let mut rest = number;
        while rest >= 0x80 {
            self.0
                .push(u8::try_from(rest & 0x7f).expect("seven bits fit a byte") | 0x80);
            rest >>= 7;
        }
        self.0.push(u8::try_from(rest).expect("below 0x80"));
    }
}
