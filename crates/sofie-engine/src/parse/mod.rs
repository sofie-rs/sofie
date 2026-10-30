mod attributes;
mod call;
mod closer;
mod component;
pub(crate) mod cursor;
pub(crate) mod expression;
mod ids;
mod program;
mod rules;
mod scope;

use self::attributes::{Deferred, parse_attributes, parse_attributes_deferring};
pub(crate) use self::call::each_call;
use self::closer::{Closer, Opener, close, is_at_closer, read_closer};
use self::cursor::{Cursor, is_identifier_byte, is_identifier_start};
use self::expression::{is_root_name, parse_braced, parse_expression};
use self::ids::Ids;
use self::scope::Scope;
use crate::catalog::Kind;
use crate::components::Components;
use crate::syntax::{
    Attribute, Conditional, Expression, LocatedError, Loop, MAX_DEPTH, Op, Position, Written,
};

type Program = crate::program::Program<Written>;

pub(crate) fn parse(
    source: &str,
    components: &Components,
) -> Result<Vec<Op<Written>>, LocatedError> {
    Parser::new(source, components, Vec::new(), 0)
        .run()
        .map(|(ops, _)| ops)
}

#[cfg(feature = "analysis")]
pub(crate) fn parse_recorded(
    source: &str,
    components: &Components,
) -> Result<crate::analysis::Recorder, LocatedError> {
    Parser::new(source, components, Vec::new(), 0)
        .run()
        .map(|(_, scope)| scope.recorder)
}

#[derive(Clone, Copy, Default)]
struct PageNames {
    title: bool,
    head: bool,
    head_title: bool,
}

impl PageNames {
    /// The page moves the first `<title>` of a page's markup outside an `<svg>` into its head, so
    /// it is written once, where nothing else can write one first.
    fn claim_title(
        &mut self,
        depth: usize,
        attributes: &[Attribute],
        position: Position,
    ) -> Result<(), LocatedError> {
        if depth != 1 || self.title || !attributes.is_empty() {
            bail!(
                position,
                "`<title>` names the page: write it once, without attributes, at the template's \
                 top level"
            );
        }
        self.title = true;
        self.refuse_two(position)
    }

    /// The page moves only the first `<Head>`'s tags into its head; a second would leave its
    /// tags in the body.
    #[cfg(feature = "content")]
    fn claim_head(&mut self, has_title: bool, position: Position) -> Result<(), LocatedError> {
        if self.head {
            bail!(
                position,
                "`<Head>` is written twice; a page has one head, so write it once"
            );
        }
        self.head = true;
        self.head_title = has_title;
        self.refuse_two(position)
    }

    /// An `{#if}`'s branches may each name the page.
    fn join(&mut self, other_branch: Self) {
        self.title |= other_branch.title;
        self.head |= other_branch.head;
        self.head_title |= other_branch.head_title;
    }

    fn refuse_two(self, position: Position) -> Result<(), LocatedError> {
        if self.title && self.head_title {
            bail!(
                position,
                "the page is named twice: keep either `<title>` or `<Head title=…>`"
            );
        }
        Ok(())
    }
}

struct Parser<'s> {
    cursor: Cursor<'s>,
    scope: Scope,
    depth: usize,
    is_in_form: bool,
    names: PageNames,
    is_in_title: bool,
    is_in_svg: bool,
    ids: Ids,
    components: &'s Components,
    /// The components being inlined around this one, so none writes itself.
    expanding: Vec<String>,
}

impl<'s> Parser<'s> {
    fn new(
        source: &'s str,
        components: &'s Components,
        expanding: Vec<String>,
        depth: usize,
    ) -> Self {
        Self {
            cursor: Cursor::new(source),
            scope: Scope::default(),
            depth,
            is_in_form: false,
            names: PageNames::default(),
            is_in_title: false,
            is_in_svg: false,
            ids: Ids::default(),
            components,
            expanding,
        }
    }

    fn run(mut self) -> Result<(Vec<Op<Written>>, Scope), LocatedError> {
        let ops = self.parse_all()?;
        Ok((ops, self.scope))
    }

    fn parse_all(&mut self) -> Result<Vec<Op<Written>>, LocatedError> {
        let program = self.parse_children()?;
        match read_closer(&mut self.cursor)? {
            None => Ok(program.into_ops()),
            Some((closer, position)) => bail!(position, "{closer} closes nothing that is open"),
        }
    }

    fn parse_children(&mut self) -> Result<Program, LocatedError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self
                .cursor
                .error(format!("nesting is deeper than {MAX_DEPTH} levels")));
        }
        let mut program = Program::default();
        while !self.cursor.is_at_end() && !is_at_closer(&self.cursor) {
            self.parse_node(&mut program)?;
        }
        self.depth -= 1;
        Ok(program)
    }

    fn parse_node(&mut self, program: &mut Program) -> Result<(), LocatedError> {
        if self.cursor.starts_with("<!--") {
            return skip_comment(&mut self.cursor);
        }
        match self.cursor.peek() {
            Some(b'<') => self.parse_tag(program),
            Some(b'{') => self.parse_brace(program),
            _ => {
                program.push_markup(self.cursor.take_while(|byte| byte != b'<' && byte != b'{'));
                Ok(())
            }
        }
    }

    fn parse_brace(&mut self, program: &mut Program) -> Result<(), LocatedError> {
        let position = self.cursor.position();
        if !self.cursor.eat("{#") {
            self.cursor.advance();
            let text = parse_braced(&mut self.cursor, self.scope.names())?;
            self.scope.record(&text, position);
            program.push(Op::Text(text));
            return Ok(());
        }
        let block = match self.cursor.take_identifier() {
            "if" => self.parse_if(position)?,
            "each" => self.parse_each(position)?,
            keyword => {
                bail!(
                    position,
                    "unknown block `{{#{keyword}}}`; blocks are `{{#if …}}` and `{{#each … as …}}`"
                );
            }
        };
        program.push(block);
        Ok(())
    }

    fn parse_if(&mut self, position: Position) -> Result<Op<Written>, LocatedError> {
        let condition = parse_braced(&mut self.cursor, self.scope.names())?;
        self.scope.record(&condition, position);
        let opener = Opener::new("`{#if}`", position);
        let ids_before = self.ids.clone();
        let names_before = self.names;
        let then_branch = self.parse_children()?.into_ops();
        let mut else_branch = Vec::new();
        if close(&mut self.cursor, &opener, &[Closer::Else, Closer::EndIf])? == Closer::Else {
            let then_ids = std::mem::replace(&mut self.ids, ids_before);
            let then_names = std::mem::replace(&mut self.names, names_before);
            else_branch = self.parse_children()?.into_ops();
            close(&mut self.cursor, &opener, &[Closer::EndIf])?;
            self.ids.join(then_ids);
            self.names.join(then_names);
        }
        Ok(Op::If(Conditional {
            condition,
            then_branch,
            else_branch,
        }))
    }

    fn parse_each(&mut self, position: Position) -> Result<Op<Written>, LocatedError> {
        let list = parse_expression(&mut self.cursor, self.scope.names())?;
        self.scope.record(&list, position);
        self.cursor.skip_whitespace();
        let keyword_position = self.cursor.position();
        if self.cursor.take_identifier() != "as" {
            bail!(
                keyword_position,
                "expected `as` after the list in `{{#each}}`"
            );
        }
        let item = parse_loop_name(&mut self.cursor)?;
        self.cursor.skip_whitespace();
        let index = if self.cursor.eat(",") {
            self.cursor.skip_whitespace();
            let position = self.cursor.position();
            let index = parse_loop_name(&mut self.cursor)?;
            if index == item {
                bail!(
                    position,
                    "the index `{index}` has the item's name; give it another, such as `index`"
                );
            }
            Some(index)
        } else {
            None
        };
        self.cursor.skip_whitespace();
        self.cursor.expect("}", "to end `{#each}`")?;
        let has_index = index.is_some();
        let outer_scope = self.scope.depth();
        self.scope.enter_loop(item, index, &list);
        let was_repeated = std::mem::replace(&mut self.ids.is_repeated, true);
        let body = self.parse_children();
        self.ids.is_repeated = was_repeated;
        self.scope.leave(outer_scope);
        let body = body?.into_ops();
        let opener = Opener::new("`{#each}`", position);
        close(&mut self.cursor, &opener, &[Closer::EndEach])?;
        Ok(Op::Each(Loop {
            list,
            has_index,
            body,
        }))
    }

    fn parse_tag(&mut self, program: &mut Program) -> Result<(), LocatedError> {
        let position = self.cursor.position();
        if self.is_in_title {
            bail!(position, "`<title>` holds only text and `{{…}}`");
        }
        self.cursor.advance();
        if !self
            .cursor
            .peek()
            .is_some_and(|byte| byte.is_ascii_alphabetic())
        {
            bail!(
                position,
                "expected a tag name after `<`, found {}; write `&lt;` for a literal `<`",
                self.cursor.describe_next()
            );
        }
        let name = self.cursor.take_while(is_tag_name_byte);
        if name.starts_with(|first: char| first.is_ascii_uppercase()) {
            if let Some(source) = self.components.source(name) {
                return self.inline_component(name, source, position, program);
            }
            program.push(self.parse_component(name, position)?);
            return Ok(());
        }
        self.parse_element(name, position, program)
    }

    fn parse_element(
        &mut self,
        tag: &str,
        position: Position,
        program: &mut Program,
    ) -> Result<(), LocatedError> {
        rules::check_element_name(tag, position)?;
        let (attributes, is_self_closing) =
            parse_attributes(&mut self.cursor, &mut self.scope, tag, position)?;
        for attribute in &attributes {
            rules::check_element_attribute(tag, attribute)?;
        }
        let is_page_title = tag == "title" && !self.is_in_svg;
        if is_page_title {
            self.names.claim_title(self.depth, &attributes, position)?;
        }
        self.ids.claim(&attributes)?;
        let children = if rules::is_void(tag) {
            None
        } else if is_self_closing {
            Some(Program::default())
        } else {
            self.is_in_title = is_page_title;
            let was_in_svg = self.is_in_svg;
            self.is_in_svg |= tag == "svg";
            let children = self.parse_contents(tag, position);
            self.is_in_title = false;
            self.is_in_svg = was_in_svg;
            Some(children?)
        };
        program.push_element(tag, attributes, children);
        Ok(())
    }

    fn parse_component(
        &mut self,
        name: &str,
        position: Position,
    ) -> Result<Op<Written>, LocatedError> {
        let Some(kind) = Kind::named(name) else {
            bail!(
                position,
                "unknown component `{name}`; the catalog has {}",
                Kind::list()
            );
        };
        if matches!(kind, Kind::Field | Kind::Submit) && !self.is_in_form {
            bail!(
                position,
                "`{name}` belongs inside a `{}`",
                Kind::Form.name()
            );
        }
        if kind == Kind::Form && self.ids.is_repeated {
            bail!(
                position,
                "a `Form` cannot be inside `{{#each}}` or a `Table` row: its id would repeat, so \
                 errors would land under the first form; post the item's fields with an \
                 `Action`, such as `<Action post=\"…\" label=\"…\" with={{{{id: item.id}}}} />`"
            );
        }
        #[cfg(feature = "content")]
        if kind == Kind::Head && self.ids.is_repeated {
            bail!(
                position,
                "a `Head` cannot be inside `{{#each}}` or a `Table` row: it would be written once \
                 per item, and a page has one head; write it once, outside the loop"
            );
        }
        let deferred_name = (kind == Kind::Table).then_some("link");
        let attributes = parse_attributes_deferring(
            &mut self.cursor,
            &mut self.scope,
            name,
            position,
            deferred_name,
        )?;
        let (mut props, is_self_closing) = (attributes.given, attributes.is_self_closing);
        self.ids.claim(&props)?;
        #[cfg(feature = "content")]
        if kind == Kind::Head {
            let has_title = props.iter().any(|prop| prop.name == "title");
            self.names.claim_head(has_title, position)?;
        }
        if kind == Kind::Action {
            self.scope.record_action(&props, position);
        }
        if kind == Kind::Table {
            self.scope.record_table_columns(&props);
        }
        #[cfg(feature = "islands")]
        if kind == Kind::Island {
            self.scope.mark_island(&props);
        }
        let bound_name = bound_name(kind, &props)?;
        if let Some(deferred) = attributes.deferred {
            let attribute = self.parse_for_rows(deferred, bound_name.as_ref(), &props)?;
            props.push(attribute);
            self.scope.record_table_link(&props, position);
        }
        let children = if is_self_closing {
            Program::default()
        } else {
            let outer_scope = self.scope.depth();
            if let Some(bound_name) = bound_name {
                self.scope.enter_rows(bound_name, &props);
            }
            let was_in_form = std::mem::replace(&mut self.is_in_form, kind == Kind::Form);
            let was_repeated = self.ids.is_repeated;
            self.ids.is_repeated |= kind == Kind::Table;
            let children = self.parse_contents(name, position);
            self.is_in_form = was_in_form;
            self.ids.is_repeated = was_repeated;
            self.scope.leave(outer_scope);
            children?
        };
        Ok(Op::Component(Written {
            kind,
            attributes: props,
            children: children.into_ops(),
            position,
        }))
    }

    fn parse_for_rows(
        &mut self,
        deferred: Deferred<'s>,
        bound_name: Option<&String>,
        props: &[Attribute],
    ) -> Result<Attribute, LocatedError> {
        let outer_scope = self.scope.depth();
        if let Some(bound_name) = bound_name {
            self.scope.enter_rows(bound_name.clone(), props);
        }
        let attribute = deferred.parse(&mut self.scope);
        self.scope.leave(outer_scope);
        attribute
    }

    fn parse_contents(&mut self, tag: &str, position: Position) -> Result<Program, LocatedError> {
        let children = self.parse_children()?;
        let opener = Opener::new(format!("`<{tag}>`"), position);
        close(&mut self.cursor, &opener, &[Closer::Tag(tag.to_owned())])?;
        Ok(children)
    }
}

fn skip_comment(cursor: &mut Cursor<'_>) -> Result<(), LocatedError> {
    let start = cursor.position();
    cursor.eat("<!--");
    while !cursor.eat("-->") {
        if cursor.is_at_end() {
            return Err(LocatedError::new(start, "comment `<!--` is never closed"));
        }
        cursor.advance();
    }
    Ok(())
}

fn parse_loop_name(cursor: &mut Cursor<'_>) -> Result<String, LocatedError> {
    cursor.skip_whitespace();
    let position = cursor.position();
    let name = cursor.take_identifier();
    if name.is_empty() {
        return Err(cursor.unexpected(format_args!("expected a loop name")));
    }
    if is_root_name(name) {
        bail!(
            position,
            "`{name}` is the root binding and cannot be a loop name"
        );
    }
    Ok(name.to_owned())
}

/// Checked even on a table without children: the parser binds `as` before reading them.
fn bound_name(kind: Kind, props: &[Attribute]) -> Result<Option<String>, LocatedError> {
    if kind != Kind::Table {
        return Ok(None);
    }
    let Some(attribute) = props.iter().find(|attribute| attribute.name == "as") else {
        return Ok(None);
    };
    match &attribute.value {
        Expression::Authored(name) if is_bindable_name(name) => Ok(Some(name.clone())),
        _ => bail!(
            attribute.position,
            "`as` needs a quoted name other than `data`, such as `as=\"order\"`"
        ),
    }
}

fn is_bindable_name(name: &str) -> bool {
    name.bytes().next().is_some_and(is_identifier_start)
        && name.bytes().all(is_identifier_byte)
        && !is_root_name(name)
}

fn is_tag_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

#[cfg(test)]
mod tests {
    use crate::syntax::{Op, Written};

    fn parse(source: &str) -> Result<Vec<Op<Written>>, crate::syntax::LocatedError> {
        super::parse(source, &crate::Components::default())
    }

    fn error_at(source: &str) -> (usize, usize) {
        let error = parse(source).expect_err("source is invalid");
        (error.position.line, error.position.column)
    }

    #[test]
    fn skips_comments_and_keeps_text() {
        let ops = parse("<!-- note -->a<br>b").expect("valid");
        assert!(matches!(ops.as_slice(), [Op::Markup(markup)] if markup == "a<br>b"));
    }

    #[test]
    fn joins_literal_markup_and_keeps_elements_with_ids_whole() {
        let ops = parse(
            "<p class=\"a&b\"><a href=\"javascript:x\">y</a></p><div id=\"d\">{data.z}</div>",
        )
        .expect("valid");
        let [Op::Markup(markup), Op::Identified { body, .. }] = ops.as_slice() else {
            panic!("expected markup and an identified element: {ops:?}");
        };
        assert_eq!(markup, "<p class=\"a&b\"><a href=\"#\">y</a></p>");
        assert!(matches!(
            body.as_slice(),
            [Op::Markup(open), Op::Text(_), Op::Markup(close)] if open == "<div id=\"d\">" && close == "</div>"
        ));
    }

    #[test]
    fn refuses_a_character_reference_that_could_spell_a_url_scheme() {
        for source in [
            "<a href=\"javascript&colon;alert(1)\">x</a>",
            "<a href=\"jav&#x61;script:alert(1)\">x</a>",
            "<img src=\"&#106;avascript:x\">",
            "<a href=\"java{data.x}&colon;y\">x</a>",
        ] {
            let refused = parse(source).and_then(crate::catalog::build_all);
            let error = refused.expect_err(source);
            assert!(
                error.message.contains("character reference"),
                "{source}: {}",
                error.message
            );
        }
        for source in [
            "<a href=\"/search?a=1&amp;b=2\">x</a>",
            "<a href=\"#a&b\">x</a>",
            "<a href=\"mailto:a@x.test?subject=a&amp;body=b\">x</a>",
            "<p title=\"a&amp;b\">x</p>",
        ] {
            assert!(parse(source).is_ok(), "{source}");
        }
    }

    #[cfg(feature = "layout")]
    #[test]
    fn a_component_s_url_prop_refuses_a_character_reference_before_its_path() {
        let ops = parse("<Card href=\"javascript&colon;x\">y</Card>").expect("parses");
        let error = crate::catalog::build_all(ops).expect_err("refused");
        assert!(
            error.message.contains("character reference"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_literal_interpolation_is_still_a_child_a_form_refuses() {
        let ops = parse("<Form id=\"f\" post=\"/x\">{\" \"}</Form>").expect("parses");
        let error = crate::catalog::build_all(ops).expect_err("refused");
        assert!(error.message.contains("takes only"), "{}", error.message);
    }

    #[test]
    fn reports_closers_at_their_position() {
        assert_eq!(error_at("<p>\n  </div>"), (2, 3));
        assert_eq!(error_at("a{/if}"), (1, 2));
        assert_eq!(error_at("<p>{#if data.a}</p>{/if}"), (1, 16));
    }

    #[test]
    fn reports_unclosed_openers_at_the_opener() {
        assert_eq!(error_at("x\n {#each data.a as b}"), (2, 2));
        assert_eq!(error_at("<!-- open"), (1, 1));
    }

    #[test]
    fn an_index_named_like_its_item_is_refused() {
        assert_eq!(error_at("{#each data.a as row, row}{/each}"), (1, 23));
    }

    #[test]
    fn bounds_nesting_depth() {
        let deep = "<div>".repeat(1000);
        assert!(parse(&deep).is_err_and(|error| error.message.contains("deeper")));
    }
}
