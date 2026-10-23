use crate::Template;
use crate::catalog::Content;
use crate::render::parts_of;
use crate::syntax::{Expression, Op};

/// A bound path part is written `{}`, as in `/account/orders/{}/cancel`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormPost {
    pub id: String,
    pub post: String,
}

impl Template {
    /// Searched as a fragment render searches; a bound id is a pattern.
    #[must_use]
    pub fn element_ids(&self) -> Vec<IdPattern> {
        let mut ids = Vec::new();
        collect(&self.ops, &mut ids);
        ids
    }

    #[must_use]
    pub fn form_posts(&self) -> Vec<FormPost> {
        let mut posts = Vec::new();
        visit_components(&self.ops, &mut |content| {
            if let Content::Form(form) = content {
                posts.push(FormPost {
                    id: form.id.clone(),
                    post: posted_path(&form.post),
                });
            }
        });
        posts
    }
}

pub(super) fn posted_path(post: &Expression) -> String {
    parts_of(post)
        .iter()
        .map(|part| match part_of(part) {
            IdPart::Text(text) => text,
            IdPart::Bound => "{}".to_owned(),
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdPattern(Vec<IdPart>);

#[derive(Clone, Debug, PartialEq, Eq)]
enum IdPart {
    Text(String),
    Bound,
}

impl IdPattern {
    #[must_use]
    pub fn could_be(&self, id: &str) -> bool {
        matches(&self.0, id)
    }

    fn of(id: &Expression) -> Self {
        Self(parts_of(id).iter().map(part_of).collect())
    }
}

fn part_of(expression: &Expression) -> IdPart {
    match expression {
        Expression::Text(text) | Expression::Authored(text) => IdPart::Text(text.clone()),
        Expression::Integer(number) => IdPart::Text(number.to_string()),
        _ => IdPart::Bound,
    }
}

fn matches(parts: &[IdPart], id: &str) -> bool {
    match parts.split_first() {
        None => id.is_empty(),
        Some((IdPart::Text(text), rest)) => id
            .strip_prefix(text.as_str())
            .is_some_and(|remainder| matches(rest, remainder)),
        Some((IdPart::Bound, rest)) => (0..=id.len())
            .filter(|&split| id.is_char_boundary(split))
            .any(|split| matches(rest, &id[split..])),
    }
}

fn collect(ops: &[Op], ids: &mut Vec<IdPattern>) {
    for op in ops {
        match op {
            Op::Markup(_) | Op::Text(_) | Op::Url(_) => {}
            Op::Identified { id, body } => {
                ids.push(IdPattern::of(id));
                collect(body, ids);
            }
            Op::Component(component) => {
                ids.extend(component.id.as_ref().map(IdPattern::of));
                collect(component.children(), ids);
            }
            Op::If(conditional) => {
                collect(&conditional.then_branch, ids);
                collect(&conditional.else_branch, ids);
            }
            Op::Each(each) => collect(&each.body, ids),
        }
    }
}

fn visit_components(ops: &[Op], visit: &mut impl FnMut(&Content)) {
    for op in ops {
        match op {
            Op::Markup(_) | Op::Text(_) | Op::Url(_) => {}
            Op::Identified { body, .. } | Op::Each(crate::syntax::Loop { body, .. }) => {
                visit_components(body, visit);
            }
            Op::Component(component) => {
                visit(&component.content);
                visit_components(component.children(), visit);
            }
            Op::If(conditional) => {
                visit_components(&conditional.then_branch, visit);
                visit_components(&conditional.else_branch, visit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Template;

    const SCREEN: &str = "<Section title=\"Orders\" id=\"orders\">\n\
        <!-- <Section id=\"commented\"></Section> -->\n\
        {#each data.orders as order}<div id=\"row-{order.id}\">x</div>{/each}\n\
        {#if data.open}<p id=\"open\">o</p>{:else}<Form id=\"reopen\" post=\"/r\"><Submit label=\"Go\" /></Form>{/if}\n\
        <p title=\"a > b\">id=\"prose\"</p>\n\
        <Table id=\"lines\" of={data.lines} columns=\"name\" as=\"line\">\n\
        <Action id=\"in-row-{line.id}\" post=\"/x/{line.id}\" label=\"x\" />\n\
        </Table>\n\
        </Section>";

    fn could_render(source: &str, id: &str) -> bool {
        let template = Template::parse("screen", source).unwrap();
        template
            .element_ids()
            .iter()
            .any(|pattern| pattern.could_be(id))
    }

    #[test]
    fn literal_ids_in_elements_components_and_both_branches_are_listed() {
        for id in ["orders", "lines", "open", "reopen"] {
            assert!(could_render(SCREEN, id), "{id}");
        }
        assert!(!could_render(SCREEN, "order"));
        assert!(!could_render(SCREEN, "orders-x"));
    }

    #[test]
    fn a_bound_part_matches_any_text() {
        assert!(could_render(SCREEN, "row-7"));
        assert!(could_render(SCREEN, "row-"));
        assert!(!could_render(SCREEN, "line-7"));
        assert!(could_render("<p id={data.dom_id}>x</p>", "anything"));
    }

    #[test]
    fn comments_text_and_table_row_actions_hold_no_findable_id() {
        for id in ["commented", "prose", "in-row-7"] {
            assert!(!could_render(SCREEN, id), "{id}");
        }
    }

    #[test]
    fn a_pattern_matches_across_multibyte_text() {
        let source = "<div id=\"ü-{data.x}-ö\">x</div>";

        assert!(could_render(source, "ü-ä-ö"));
        assert!(!could_render(source, "ü-ä"));
    }
}
