use crate::escape::push_url;
use crate::program::{Program, constant};
use crate::syntax::{Attribute, Expression, Op, Written};

const URL_ATTRIBUTES: [&str; 4] = ["href", "src", "action", "formaction"];

impl Program<Written> {
    fn append(&mut self, other: Self) {
        for op in other.into_ops() {
            self.push(op);
        }
    }

    pub fn push_element(&mut self, tag: &str, attributes: Vec<Attribute>, children: Option<Self>) {
        let id = attributes
            .iter()
            .find(|attribute| attribute.name == "id")
            .map(|attribute| attribute.value.clone());
        let mut body = Self::default();
        body.push_markup("<");
        body.push_markup(tag);
        for attribute in attributes {
            body.push_attribute(attribute);
        }
        body.push_markup(">");
        if let Some(children) = children {
            body.append(children);
            body.push_markup("</");
            body.push_markup(tag);
            body.push_markup(">");
        }
        match id {
            Some(id) => self.push(Op::Identified {
                id,
                body: body.into_ops(),
            }),
            None => self.append(body),
        }
    }

    fn push_attribute(&mut self, attribute: Attribute) {
        self.push_markup(" ");
        self.push_markup(&attribute.name);
        self.push_markup("=\"");
        if URL_ATTRIBUTES.contains(&attribute.name.as_str()) {
            self.push_url(attribute.value);
        } else {
            self.push_value(attribute.value);
        }
        self.push_markup("\"");
    }

    fn push_value(&mut self, value: Expression) {
        match value {
            Expression::Authored(markup) => self.push_markup(&markup),
            Expression::Interpolated(parts) => {
                for part in parts {
                    self.push_value(part);
                }
            }
            _ => self.push_text(value),
        }
    }

    fn push_url(&mut self, url: Expression) {
        let is_authored = matches!(url, Expression::Authored(_));
        let Some(text) = constant(&url) else {
            self.push(Op::Url(url));
            return;
        };
        push_url(&mut self.markup, &text, is_authored);
    }
}
