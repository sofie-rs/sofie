//! The parser checked the markup and wrote every element in one form (`<tag name="value">`,
//! values in double quotes, `</tag>` unless void), so reading it back needs no HTML parser.

use crate::catalog::Component;
use crate::syntax::{Conditional, Expression, Loop, Op};

#[derive(Debug)]
pub(super) enum Node<'t> {
    Element(Element<'t>),
    /// Text as the parser escaped it.
    Markup(String),
    Text(&'t Expression),
    Component(&'t Component),
    If {
        condition: &'t Expression,
        then_branch: Vec<Node<'t>>,
        else_branch: Vec<Node<'t>>,
    },
    Each {
        list: &'t Expression,
        has_index: bool,
        body: Vec<Node<'t>>,
    },
}

#[derive(Debug)]
pub(super) struct Element<'t> {
    pub tag: String,
    pub attributes: Vec<ElementAttribute<'t>>,
    /// `None` for a void element.
    pub children: Option<Vec<Node<'t>>>,
}

#[derive(Debug)]
pub(super) struct ElementAttribute<'t> {
    pub name: String,
    pub value: Vec<AttributePart<'t>>,
}

#[derive(Debug)]
pub(super) enum AttributePart<'t> {
    Markup(String),
    Text(&'t Expression),
    Url(&'t Expression),
}

pub(super) fn tree(ops: &[Op]) -> Result<Vec<Node<'_>>, String> {
    let mut builder = Builder::default();
    builder.ops(ops)?;
    builder.finish()
}

#[derive(Default)]
struct Builder<'t> {
    open: Vec<Element<'t>>,
    root: Vec<Node<'t>>,
    state: State<'t>,
}

#[derive(Default)]
enum State<'t> {
    #[default]
    Content,
    Tag(Element<'t>),
    Value(Element<'t>, ElementAttribute<'t>),
}

impl<'t> Builder<'t> {
    fn ops(&mut self, ops: &'t [Op]) -> Result<(), String> {
        for op in ops {
            self.op(op)?;
        }
        Ok(())
    }

    fn op(&mut self, op: &'t Op) -> Result<(), String> {
        match op {
            Op::Markup(markup) => self.markup(markup),
            Op::Text(expression) => {
                self.expression(expression, AttributePart::Text);
                Ok(())
            }
            Op::Url(expression) => {
                self.expression(expression, AttributePart::Url);
                Ok(())
            }
            Op::Identified { body, .. } => self.ops(body),
            Op::Component(component) => self.node(Node::Component(component)),
            Op::If(Conditional {
                condition,
                then_branch,
                else_branch,
            }) => {
                let node = Node::If {
                    condition,
                    then_branch: tree(then_branch)?,
                    else_branch: tree(else_branch)?,
                };
                self.node(node)
            }
            Op::Each(Loop {
                list,
                has_index,
                body,
            }) => {
                let node = Node::Each {
                    list,
                    has_index: *has_index,
                    body: tree(body)?,
                };
                self.node(node)
            }
        }
    }

    fn expression(
        &mut self,
        expression: &'t Expression,
        part: fn(&'t Expression) -> AttributePart<'t>,
    ) {
        match &mut self.state {
            State::Value(_, attribute) => attribute.value.push(part(expression)),
            State::Content | State::Tag(_) => self.push(Node::Text(expression)),
        }
    }

    fn node(&mut self, node: Node<'t>) -> Result<(), String> {
        if !matches!(self.state, State::Content) {
            return Err("a block or component inside a tag".to_owned());
        }
        self.push(node);
        Ok(())
    }

    fn push(&mut self, node: Node<'t>) {
        let siblings = match self.open.last_mut() {
            Some(element) => element.children.get_or_insert_with(Vec::new),
            None => &mut self.root,
        };
        if let (Node::Markup(text), Some(Node::Markup(last))) = (&node, siblings.last_mut()) {
            last.push_str(text);
            return;
        }
        siblings.push(node);
    }

    fn markup(&mut self, mut rest: &str) -> Result<(), String> {
        while !rest.is_empty() {
            rest = match std::mem::take(&mut self.state) {
                State::Content => self.content(rest)?,
                State::Tag(element) => self.tag(element, rest)?,
                State::Value(element, attribute) => self.value(element, attribute, rest),
            };
        }
        Ok(())
    }

    fn content<'m>(&mut self, markup: &'m str) -> Result<&'m str, String> {
        let Some(start) = markup.find('<') else {
            self.push(Node::Markup(markup.to_owned()));
            return Ok("");
        };
        if start > 0 {
            self.push(Node::Markup(markup[..start].to_owned()));
        }
        let rest = &markup[start + 1..];
        if let Some(closing) = rest.strip_prefix('/') {
            let end = closing.find('>').ok_or("an unended closing tag")?;
            self.close(&closing[..end])?;
            return Ok(&closing[end + 1..]);
        }
        let end = rest.find([' ', '>']).ok_or("an unended opening tag")?;
        self.state = State::Tag(Element {
            tag: rest[..end].to_owned(),
            attributes: Vec::new(),
            children: Some(Vec::new()),
        });
        Ok(&rest[end..])
    }

    fn tag<'m>(&mut self, element: Element<'t>, markup: &'m str) -> Result<&'m str, String> {
        if let Some(rest) = markup.strip_prefix('>') {
            self.open_element(element);
            return Ok(rest);
        }
        let rest = markup
            .strip_prefix(' ')
            .ok_or_else(|| format!("unexpected `{markup}` in `<{}>`", element.tag))?;
        let end = rest
            .find("=\"")
            .ok_or_else(|| format!("an attribute without a value in `<{}>`", element.tag))?;
        let attribute = ElementAttribute {
            name: rest[..end].to_owned(),
            value: Vec::new(),
        };
        self.state = State::Value(element, attribute);
        Ok(&rest[end + 2..])
    }

    fn value<'m>(
        &mut self,
        mut element: Element<'t>,
        mut attribute: ElementAttribute<'t>,
        markup: &'m str,
    ) -> &'m str {
        let Some(end) = markup.find('"') else {
            attribute
                .value
                .push(AttributePart::Markup(markup.to_owned()));
            self.state = State::Value(element, attribute);
            return "";
        };
        if end > 0 {
            attribute
                .value
                .push(AttributePart::Markup(markup[..end].to_owned()));
        }
        element.attributes.push(attribute);
        self.state = State::Tag(element);
        &markup[end + 1..]
    }

    fn open_element(&mut self, mut element: Element<'t>) {
        if is_void(&element.tag) {
            element.children = None;
            self.push(Node::Element(element));
            return;
        }
        self.open.push(element);
    }

    fn close(&mut self, tag: &str) -> Result<(), String> {
        let element = self
            .open
            .pop()
            .filter(|element| element.tag == tag)
            .ok_or_else(|| format!("`</{tag}>` closes no open element"))?;
        self.push(Node::Element(element));
        Ok(())
    }

    fn finish(self) -> Result<Vec<Node<'t>>, String> {
        if let Some(element) = self.open.last() {
            return Err(format!("`<{}>` is never closed", element.tag));
        }
        if !matches!(self.state, State::Content) {
            return Err("the markup ends inside a tag".to_owned());
        }
        Ok(self.root)
    }
}

fn is_void(tag: &str) -> bool {
    [
        "area", "br", "col", "hr", "img", "input", "source", "track", "wbr",
    ]
    .contains(&tag)
}

#[cfg(test)]
mod tests {
    use super::{AttributePart, Node, tree};
    use crate::syntax::Op;

    fn ops(source: &str) -> Vec<Op> {
        let written = crate::parse::parse(source, &crate::Components::default()).unwrap();
        crate::catalog::build_all(written).unwrap()
    }

    #[test]
    fn rebuilds_elements_text_and_attributes_with_their_parts() {
        let ops = ops("<p class=\"a {data.b} c\">x {data.y}<br><a href={data.u}>z</a></p>");
        let nodes = tree(&ops).unwrap();

        let [Node::Element(paragraph)] = nodes.as_slice() else {
            panic!("{nodes:?}");
        };
        assert_eq!(paragraph.tag, "p");
        assert_eq!(paragraph.attributes[0].name, "class");
        assert!(matches!(
            paragraph.attributes[0].value.as_slice(),
            [AttributePart::Markup(a), AttributePart::Text(_), AttributePart::Markup(c)]
                if a == "a " && c == " c"
        ));
        let children = paragraph.children.as_ref().unwrap();
        assert!(matches!(
            children.as_slice(),
            [Node::Markup(x), Node::Text(_), Node::Element(br), Node::Element(link)]
                if x == "x " && br.children.is_none() && link.tag == "a"
        ));
        let Node::Element(link) = &children[3] else {
            unreachable!()
        };
        assert!(matches!(
            link.attributes[0].value.as_slice(),
            [AttributePart::Url(_)]
        ));
    }

    #[test]
    fn keeps_blocks_and_components_as_nodes() {
        let ops = ops(
            "<div id=\"d\">{#if data.a}<b>1</b>{:else}2{/if}{#each data.l as i, n}{i}{/each}\
             <Section title=\"T\"><i>s</i></Section></div>",
        );
        let nodes = tree(&ops).unwrap();

        let [Node::Element(div)] = nodes.as_slice() else {
            panic!("{nodes:?}");
        };
        assert!(matches!(
            div.children.as_deref().unwrap(),
            [
                Node::If { then_branch, else_branch, .. },
                Node::Each { has_index: true, .. },
                Node::Component(_),
            ] if then_branch.len() == 1 && else_branch.len() == 1
        ));
    }

    #[test]
    fn a_greater_than_sign_in_a_value_stays_in_it() {
        let ops = ops("<span title=\"a > b\">c</span>");
        let nodes = tree(&ops).unwrap();

        let [Node::Element(span)] = nodes.as_slice() else {
            panic!("{nodes:?}");
        };
        assert!(matches!(
            span.attributes[0].value.as_slice(),
            [AttributePart::Markup(text)] if text == "a > b"
        ));
    }
}
