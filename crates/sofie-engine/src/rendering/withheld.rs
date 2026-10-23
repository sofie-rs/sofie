//! Data the template binds only inside parts a viewer may not see is not sent to them either.
//! Fail closed: where a viewer fails a gated read, only what they are shown of that part of the
//! data is sent. Everything else is written `null`, so a record keeps its keys and stays truthy on
//! both sides, or as whether it holds anything where only a condition the viewer sees tests it.

use serde_json::Value;
use sofie_core::Show;
use sofie_core::listing::{ITEMS, LISTED_FIELDS};

use super::{Gate, Gates};
use crate::render::is_truthy;
use crate::slots::{Slots, Step};
use crate::syntax::{Expression, Op};
use crate::walk::{Visit, walk};

const ROOT: usize = 0;

/// The template's bound paths as a tree; empty where nothing is gated.
#[derive(Debug, Default)]
pub(crate) struct GatedPaths {
    nodes: Vec<Node>,
    gate_sets: Vec<Vec<Gate>>,
}

#[derive(Debug)]
struct Node {
    /// `None` at the root.
    step: Option<Step>,
    children: Vec<usize>,
    /// Each read with the index of the gates around it, all of which must hold.
    reads: Vec<(Read, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Read {
    /// Everything under it is sent.
    Whole = 1,
    Presence = 2,
    /// Looped over: the list keeps its length.
    Counted = 4,
}

#[derive(Clone, Copy, Debug, Default)]
struct Reads(u8);

impl Reads {
    fn add(&mut self, read: Read) {
        self.0 |= read as u8;
    }

    fn has(self, read: Read) -> bool {
        self.0 & read as u8 != 0
    }

    fn any(self) -> bool {
        self.0 != 0
    }
}

/// How one viewer sees a node: its own reads, and whether any read below it is shown or hidden.
#[derive(Clone, Copy, Debug, Default)]
struct Seen {
    shown: Reads,
    hidden: Reads,
    shown_below: bool,
    hidden_below: bool,
}

/// What one viewer is not sent of a template's data.
#[derive(Debug)]
pub struct Withheld<'t> {
    paths: &'t GatedPaths,
    /// Empty where every gate holds.
    seen: Vec<Seen>,
}

impl GatedPaths {
    pub(crate) fn of(ops: &[Op]) -> Self {
        let mut bound = Bound::default();
        walk(ops, &mut bound);
        if bound
            .bindings
            .iter()
            .all(|binding| binding.gates.is_empty())
        {
            return Self::default();
        }
        let mut paths = Self {
            nodes: vec![Node::new(None)],
            gate_sets: Vec::new(),
        };
        for binding in bound.bindings {
            let node = paths.node(&binding.steps);
            let gates = paths.gate_set(&binding.gates);
            paths.nodes[node].reads.push((binding.read, gates));
        }
        paths
    }

    fn node(&mut self, steps: &[Step]) -> usize {
        let mut index = ROOT;
        for step in steps {
            index = self.child(index, step).unwrap_or_else(|| {
                self.nodes.push(Node::new(Some(step.clone())));
                let child = self.nodes.len() - 1;
                self.nodes[index].children.push(child);
                child
            });
        }
        index
    }

    fn child(&self, index: usize, step: &Step) -> Option<usize> {
        self.child_where(index, |reached| reached == step)
    }

    fn field_child(&self, index: usize, name: &str) -> Option<usize> {
        self.child_where(
            index,
            |reached| matches!(reached, Step::Field(field) if field == name),
        )
    }

    fn child_where(&self, index: usize, reached: impl Fn(&Step) -> bool) -> Option<usize> {
        self.nodes[index]
            .children
            .iter()
            .copied()
            .find(|child| self.nodes[*child].step.as_ref().is_some_and(&reached))
    }

    fn gate_set(&mut self, gates: &[&Gate]) -> usize {
        let owned: Vec<Gate> = gates.iter().map(|gate| (*gate).clone()).collect();
        if let Some(index) = self.gate_sets.iter().position(|set| *set == owned) {
            return index;
        }
        self.gate_sets.push(owned);
        self.gate_sets.len() - 1
    }

    pub(crate) fn withheld(&self, gates: Option<&dyn Gates>) -> Withheld<'_> {
        let holds: Vec<bool> = self
            .gate_sets
            .iter()
            .map(|set| set.iter().all(|gate| gate.holds(gates)))
            .collect();
        let seen = if holds.iter().all(|holds| *holds) {
            Vec::new()
        } else {
            self.seen(&holds)
        };
        Withheld { paths: self, seen }
    }

    /// A child is always pushed after its parent, so walking backwards sees children first.
    fn seen(&self, holds: &[bool]) -> Vec<Seen> {
        let mut seen = vec![Seen::default(); self.nodes.len()];
        for (index, node) in self.nodes.iter().enumerate().rev() {
            let mut here = Seen::default();
            for (read, gates) in &node.reads {
                let reads = if holds[*gates] {
                    &mut here.shown
                } else {
                    &mut here.hidden
                };
                reads.add(*read);
            }
            for child in &node.children {
                let below = seen[*child];
                here.shown_below |= below.shown.any() || below.shown_below;
                here.hidden_below |= below.hidden.any() || below.hidden_below;
            }
            seen[index] = here;
        }
        seen
    }
}

impl Node {
    fn new(step: Option<Step>) -> Self {
        Self {
            step,
            children: Vec::new(),
            reads: Vec::new(),
        }
    }
}

impl Withheld<'_> {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }

    pub fn apply_to(&self, data: &mut Value) {
        if !self.is_empty() {
            self.withhold(ROOT, data, false);
        }
    }

    /// `enclosed` is under a whole read the viewer fails, so nothing below it is sent unshown.
    /// A part of the data with no failed read in or around it is sent untouched.
    fn withhold(&self, index: usize, value: &mut Value, enclosed: bool) {
        let seen = self.seen[index];
        if seen.shown.has(Read::Whole) {
            return;
        }
        let enclosed = enclosed || seen.hidden.has(Read::Whole);
        let failed = enclosed || seen.hidden.any();
        if !failed && !seen.hidden_below {
            return;
        }
        // The data's root stays a record, as a client screen reads it.
        if failed && !seen.shown_below && index != ROOT {
            *value = withheld_value(seen, value);
            return;
        }
        self.keep_shown(index, value, enclosed);
    }

    fn keep_shown(&self, index: usize, value: &mut Value, enclosed: bool) {
        let item = self.paths.child(index, &Step::Item);
        if let Value::Object(fields) = value {
            for (name, field) in fields {
                self.keep_field(index, item, name, field, enclosed);
            }
            return;
        }
        match item {
            Some(item) if value.is_array() => self.keep_items(item, value, enclosed),
            _ => *value = withheld_value(self.seen[index], value),
        }
    }

    fn keep_field(
        &self,
        index: usize,
        item: Option<usize>,
        name: &str,
        field: &mut Value,
        enclosed: bool,
    ) {
        if let Some(child) = self.paths.field_child(index, name) {
            return self.withhold(child, field, enclosed);
        }
        match item {
            Some(item) if name == ITEMS => self.keep_items(item, field, enclosed),
            _ => *field = Value::Null,
        }
    }

    /// A list keeps its length only where a shown read counts its items.
    fn keep_items(&self, item: usize, list: &mut Value, enclosed: bool) {
        match list {
            Value::Array(items) if self.seen[item].shown.any() => {
                for value in items {
                    self.withhold(item, value, enclosed);
                }
            }
            Value::Array(_) => *list = Value::Null,
            other => self.withhold(item, other, enclosed),
        }
    }
}

fn withheld_value(seen: Seen, value: &Value) -> Value {
    if seen.shown.has(Read::Presence) {
        Value::Bool(is_truthy(Some(value.show())))
    } else {
        Value::Null
    }
}

struct Binding<'t> {
    steps: Vec<Step>,
    read: Read,
    gates: Vec<&'t Gate>,
}

#[derive(Default)]
struct Bound<'t> {
    open: Vec<&'t Gate>,
    slots: Slots,
    bindings: Vec<Binding<'t>>,
}

impl<'t> Visit<'t> for Bound<'t> {
    fn expression(&mut self, expression: &'t Expression) {
        match expression {
            Expression::Path(path) => self.bind(self.slots.steps_of(path), Read::Whole),
            Expression::Interpolated(parts) => {
                for part in parts {
                    self.expression(part);
                }
            }
            Expression::Call(call) => self.call(call),
            _ => {}
        }
    }

    fn presence(&mut self, expression: &'t Expression) {
        match expression {
            Expression::Path(path) => self.bind(self.slots.steps_of(path), Read::Presence),
            _ => self.expression(expression),
        }
    }

    fn looped(&mut self, list: &'t Expression) {
        let Some(items) = self.rows_of(list) else {
            return;
        };
        self.bind(Some(items[..items.len() - 1].to_vec()), Read::Presence);
        self.bind(Some(items), Read::Counted);
    }

    fn enter_items(&mut self, list: &'t Expression, has_index: bool) {
        self.slots.push_items(list);
        if has_index {
            self.slots.push_dataless();
        }
    }

    fn leave_items(&mut self, has_index: bool) {
        let depth = self.slots.depth() - 1 - usize::from(has_index);
        self.slots.truncate(depth);
    }

    /// A `Table` of `data.orders.items` shows a plain list, so it reads no page keys.
    fn columns(&mut self, rows: &'t Expression, columns: &'t [String]) {
        let Some(reached) = self.slots.items_of(rows) else {
            return;
        };
        let of_items =
            matches!(reached.iter().rev().nth(1), Some(Step::Field(name)) if name == ITEMS);
        let items = rows_reached_alike(reached);
        if !of_items {
            self.bind_page_keys(&items[..items.len() - 1]);
        }
        for column in columns {
            let mut steps = items.clone();
            steps.push(Step::Field(column.clone()));
            self.bind(Some(steps), Read::Whole);
        }
    }

    fn enter_gated(&mut self, gate: &'t Gate) {
        self.open.push(gate);
    }

    fn leave_gated(&mut self) {
        self.open.pop();
    }
}

impl Bound<'_> {
    fn bind_page_keys(&mut self, listing: &[Step]) {
        for key in LISTED_FIELDS {
            let mut steps = listing.to_vec();
            steps.push(Step::Field(key.to_owned()));
            self.bind(Some(steps), Read::Whole);
        }
    }

    fn rows_of(&self, list: &Expression) -> Option<Vec<Step>> {
        self.slots.items_of(list).map(rows_reached_alike)
    }

    /// `data.orders.items` is the list of rows a `Table` of `data.orders` shows, so it is read
    /// as those rows: whole, each of them is; tested, they are counted.
    fn bind(&mut self, steps: Option<Vec<Step>>, read: Read) {
        let Some(mut steps) = steps.map(rows_reached_alike) else {
            return;
        };
        let mut read = read;
        if matches!(steps.last(), Some(Step::Field(name)) if name == ITEMS) {
            steps.pop();
            steps.push(Step::Item);
            if read == Read::Presence {
                read = Read::Counted;
            }
        }
        self.bindings.push(Binding {
            steps,
            read,
            gates: self.open.clone(),
        });
    }
}

/// `{#each data.orders.items as order}` loops over the rows a `Table` of `data.orders` shows.
fn rows_reached_alike(steps: Vec<Step>) -> Vec<Step> {
    let mut alike: Vec<Step> = Vec::with_capacity(steps.len());
    for step in steps {
        if step == Step::Item && matches!(alike.last(), Some(Step::Field(name)) if name == ITEMS) {
            alike.pop();
        }
        alike.push(step);
    }
    alike
}

/// A path whose value some viewer may be sent otherwise than it is, so a client's types can say
/// so.
#[cfg(feature = "analysis")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Withholdable {
    pub steps: Vec<Step>,
    /// The value itself may be sent `null`, or as a boolean where `tested`.
    pub nullable: bool,
    /// Sent as whether it holds anything where only a condition the viewer sees tests it.
    pub tested: bool,
    /// The fields of the value a read names or a `Table` shows; any other may be sent `null`.
    pub named_fields: Vec<String>,
}

/// The data paths a gate may withhold from some viewer, for a client's types.
#[cfg(feature = "analysis")]
#[must_use]
pub fn withholdable(template: &crate::Template) -> Vec<Withholdable> {
    template.precomputed.gated_paths.withholdable()
}

#[cfg(feature = "analysis")]
impl GatedPaths {
    fn withholdable(&self) -> Vec<Withholdable> {
        let mut found = Vec::new();
        if !self.nodes.is_empty() {
            self.collect_withholdable(ROOT, &mut Vec::new(), &mut Vec::new(), &mut found);
        }
        found
    }

    /// `enclosing` holds the gates of each whole read above, any of which a viewer may fail.
    fn collect_withholdable(
        &self,
        index: usize,
        steps: &mut Vec<Step>,
        enclosing: &mut Vec<usize>,
        found: &mut Vec<Withholdable>,
    ) {
        let node = &self.nodes[index];
        let ungated = |gates: usize| self.gate_sets[gates].is_empty();
        if node
            .reads
            .iter()
            .any(|(read, gates)| *read == Read::Whole && ungated(*gates))
        {
            return;
        }
        if enclosing.is_empty() && !self.any_read_within(index, &|gates| !ungated(gates)) {
            return;
        }
        let gated_here = node.reads.iter().any(|(_, gates)| !ungated(*gates));
        let shown_below = node
            .children
            .iter()
            .any(|child| self.any_read_within(*child, &ungated));
        let nullable = (gated_here || !enclosing.is_empty()) && !shown_below;
        found.push(Withholdable {
            steps: steps.clone(),
            nullable,
            tested: nullable && self.tested(node, enclosing),
            named_fields: self.named_fields(index),
        });
        let depth = enclosing.len();
        enclosing.extend(
            node.reads
                .iter()
                .filter(|(read, gates)| *read == Read::Whole && !ungated(*gates))
                .map(|(_, gates)| *gates),
        );
        for child in &node.children {
            if let Some(step) = &self.nodes[*child].step {
                steps.push(step.clone());
                self.collect_withholdable(*child, steps, enclosing, found);
                steps.pop();
            }
        }
        enclosing.truncate(depth);
    }

    fn any_read_within(&self, index: usize, gated: &dyn Fn(usize) -> bool) -> bool {
        let node = &self.nodes[index];
        node.reads.iter().any(|(_, gates)| gated(*gates))
            || node
                .children
                .iter()
                .any(|child| self.any_read_within(*child, gated))
    }

    /// Whether a test of the value can hold while a read of it or around it fails.
    fn tested(&self, node: &Node, enclosing: &[usize]) -> bool {
        let outside = |gates: usize, test: usize| {
            self.gate_sets[gates]
                .iter()
                .any(|gate| !self.gate_sets[test].contains(gate))
        };
        node.reads
            .iter()
            .filter(|(read, _)| *read == Read::Presence)
            .any(|(_, test)| {
                node.reads
                    .iter()
                    .map(|(_, gates)| *gates)
                    .chain(enclosing.iter().copied())
                    .any(|gates| outside(gates, *test))
            })
    }

    fn named_fields(&self, index: usize) -> Vec<String> {
        let ungated = |index: usize| {
            self.nodes[index]
                .reads
                .iter()
                .any(|(_, gates)| self.gate_sets[*gates].is_empty())
        };
        self.nodes[index]
            .children
            .iter()
            .filter_map(|child| match &self.nodes[*child].step {
                Some(Step::Field(name)) => Some(name.clone()),
                Some(Step::Item) => ungated(*child).then(|| ITEMS.to_owned()),
                None => None,
            })
            .collect()
    }
}

#[cfg(not(feature = "i18n"))]
impl<'t> Bound<'t> {
    fn call(&mut self, call: &'t crate::language::Call) {
        for value in call.values() {
            self.expression(value);
        }
    }
}

#[cfg(feature = "i18n")]
impl<'t> Bound<'t> {
    fn call(&mut self, call: &'t crate::translations::Call) {
        use crate::translations::Call;
        match call {
            Call::Translate { arguments, .. } => {
                for argument in arguments {
                    self.expression(&argument.value);
                }
            }
            Call::Format { value, .. } => self.expression(value),
            Call::Label { .. } => {}
        }
    }
}

#[cfg(all(test, feature = "parse"))]
mod tests {
    use serde_json::{Value, json};

    use crate::Template;
    use crate::rendering::{Gates, Rendering};

    struct Viewer {
        permissions: &'static [&'static str],
    }

    impl Gates for Viewer {
        fn can(&self, permission: &str) -> bool {
            self.permissions.contains(&permission)
        }

        fn enabled(&self, flag: &str) -> bool {
            flag == "order.split_payments"
        }
    }

    const COST: &str = "order.order.cost.read";

    fn sent(source: &str, permissions: &'static [&'static str], mut data: Value) -> Value {
        let template = Template::parse("screen", source).unwrap();
        let viewer = Viewer { permissions };
        Rendering::new(&template)
            .gates(&viewer)
            .withheld()
            .apply_to(&mut data);
        data
    }

    const SCREEN: &str = "<p>{data.name}</p>\
        {#if can(\"order.order.total.read\")}<p>{data.total} {data.name}</p>{/if}\
        {#if can(\"order.order.notes.read\")}{#if enabled(\"order.split_payments\")}\
        <p>{data.notes}</p>{/if}{:else}<p>{data.hint}</p>{/if}";

    #[test]
    fn a_field_bound_only_inside_parts_the_viewer_may_not_see_or_bound_nowhere_is_null() {
        let data = || json!({ "name": "a", "total": 1, "notes": "n", "hint": "h", "free": 2 });
        let both = &["order.order.total.read", "order.order.notes.read"];

        assert_eq!(
            sent(SCREEN, &[], data()),
            json!({ "name": "a", "total": null, "notes": null, "hint": "h", "free": null })
        );
        assert_eq!(
            sent(SCREEN, &["order.order.total.read"], data())["total"],
            1
        );
        assert_eq!(sent(SCREEN, both, data()), data());
    }

    #[test]
    fn a_field_bound_in_two_gated_parts_is_sent_where_either_holds() {
        let source = "{#if can(\"order.order.total.read\")}{data.total}{/if}\
            {#if can(\"order.order.*.read\")}{data.total}{/if}";

        assert_eq!(
            sent(source, &[], json!({ "total": 1 })),
            json!({ "total": null })
        );
        assert_eq!(
            sent(source, &["order.order.*.read"], json!({ "total": 1 }))["total"],
            1
        );
    }

    #[test]
    fn an_action_shown_only_through_a_gate_withholds_what_it_binds() {
        let source = "<Action post=\"/orders/{data.id}/refund\" label=\"Refund\" \
            when={can(\"order.order.*.refund\")} />";

        assert_eq!(sent(source, &[], json!({ "id": 1 })), json!({ "id": null }));
    }

    #[test]
    fn an_action_s_posted_fields_are_sent_only_where_it_is_shown() {
        let source = "{#each data.slots as slot}<Action post=\"/slots/book\" label=\"Book\" \
            with={{slot: slot.id}} />{#if can(\"order.order.cost.read\")}<Action \
            post=\"/slots/hold\" label=\"Hold\" with={{code: slot.code}} />{/if}{/each}";
        let data = || json!({ "slots": [{ "id": 1, "code": "c" }] });

        assert_eq!(
            sent(source, &[], data()),
            json!({ "slots": [{ "id": 1, "code": null }] })
        );
        assert_eq!(sent(source, &[COST], data()), data());
    }

    #[test]
    fn a_field_bound_whole_outside_a_gate_sends_what_it_holds() {
        let source =
            "<p>{data.order}</p>{#if can(\"order.order.cost.read\")}{data.order.cost}{/if}";
        let data = json!({ "order": { "cost": 2 } });

        assert_eq!(sent(source, &[], data.clone()), data);
    }

    #[test]
    fn markdown_and_head_bind_their_fields() {
        let gated = "{#if can(\"order.order.cost.read\")}<Markdown text={data.secret} />\
            <Head title={data.title} description={data.summary} />{/if}";
        let shown = "<Markdown text={data.body} /><Head title={data.title} image={data.image} />\
            {#if can(\"order.order.cost.read\")}{data.body}{data.title}{data.image}{/if}";
        let data = json!({ "secret": 1, "title": 2, "summary": 3, "body": 4, "image": 5 });

        assert_eq!(
            sent(gated, &[], data.clone()),
            json!({ "secret": null, "title": null, "summary": null, "body": null, "image": null })
        );
        assert_eq!(
            sent(shown, &[], data),
            json!({ "secret": null, "title": 2, "summary": null, "body": 4, "image": 5 })
        );
    }

    #[test]
    fn a_gated_field_of_each_item_is_withheld_by_its_full_path() {
        let source = "{#if data.orders}{#each data.orders as order, index}<p>{order.id}</p>\
            {#if can(\"order.order.cost.read\")}<p>{order.cost} {index}</p>{/if}{/each}{/if}";
        let data = || json!({ "orders": [{ "id": 1, "cost": 2 }] });

        assert_eq!(
            sent(source, &[], data()),
            json!({ "orders": [{ "id": 1, "cost": null }] })
        );
        assert_eq!(sent(source, &[COST], data()), data());
    }

    #[test]
    fn a_table_column_shown_only_through_a_gate_is_withheld() {
        let source = "{#if can(\"order.order.cost.read\")}\
            <Table id=\"all\" of={data.orders} columns=\"id cost\" />\
            {:else}<Table id=\"few\" of={data.orders} columns=\"id\" />{/if}";
        let data = || json!({ "orders": { "items": [{ "id": 1, "cost": 2 }], "page": 1 } });

        assert_eq!(
            sent(source, &[], data()),
            json!({ "orders": { "items": [{ "id": 1, "cost": null }], "page": 1 } })
        );
        assert_eq!(sent(source, &[COST], data()), data());
    }

    #[test]
    fn rows_looped_over_through_items_are_the_rows_a_table_shows() {
        let source = "<Table id=\"all\" of={data.orders} columns=\"id\" />\
            {#if can(\"order.order.cost.read\")}\
            {#each data.orders.items as order}{order.id}{order.cost}{/each}{/if}";
        let data = json!({ "orders": { "items": [{ "id": 1, "cost": 2 }], "page": 1 } });

        assert_eq!(
            sent(source, &[], data),
            json!({ "orders": { "items": [{ "id": 1, "cost": null }], "page": 1 } })
        );
    }

    #[test]
    fn a_record_whose_fields_are_all_withheld_keeps_its_keys() {
        let source = "{#if data.order}<p>shown</p>{/if}\
            {#if can(\"order.order.cost.read\")}<p>{data.order.cost}</p>{/if}";

        assert_eq!(
            sent(source, &[], json!({ "order": { "cost": 2 } })),
            json!({ "order": { "cost": null } })
        );
    }

    #[test]
    fn an_item_read_whole_inside_a_gate_keeps_only_what_is_shown_outside_it() {
        let source = "{#each data.charts as chart}<p>{chart.title}</p>\
            {#if can(\"order.order.cost.read\")}<p>{chart}</p>{/if}{/each}";
        let data = || json!({ "charts": [{ "title": "a", "points": [1], "owner": { "id": 2 } }] });

        assert_eq!(
            sent(source, &[], data()),
            json!({ "charts": [{ "title": "a", "points": null, "owner": null }] })
        );
        assert_eq!(sent(source, &[COST], data()), data());
    }

    #[test]
    fn the_data_read_whole_inside_a_gate_sends_nothing_else() {
        let source = "{#if can(\"order.order.cost.read\")}<p>{data}</p>{/if}";
        let shown = "<p>{data.name}</p>{#if can(\"order.order.cost.read\")}<p>{data}</p>{/if}";
        let data = || json!({ "name": "a", "order": { "cost": 2 } });

        assert_eq!(
            sent(source, &[], data()),
            json!({ "name": null, "order": null })
        );
        assert_eq!(
            sent(shown, &[], data()),
            json!({ "name": "a", "order": null })
        );
        assert_eq!(sent(source, &[COST], data()), data());
    }

    #[test]
    fn a_value_only_tested_outside_a_gate_is_sent_as_whether_it_holds_anything() {
        let source = "{#if data.discount}<p>off</p>{/if}\
            {#if can(\"order.order.cost.read\")}<p>{data.discount}</p>{/if}";

        assert_eq!(
            sent(source, &[], json!({ "discount": 5 })),
            json!({ "discount": true })
        );
        assert_eq!(
            sent(source, &[], json!({ "discount": "" })),
            json!({ "discount": false })
        );
        assert_eq!(
            sent(source, &[], json!({ "discount": {} })),
            json!({ "discount": false })
        );
        assert_eq!(
            sent(source, &[COST], json!({ "discount": 5 }))["discount"],
            5
        );
    }

    #[cfg(feature = "islands")]
    #[test]
    fn an_island_given_an_item_inside_a_gate_is_given_only_what_is_shown_outside() {
        let source = "{#each data.charts as chart}<p>{chart.title}</p>\
            {#if can(\"order.order.cost.read\")}<Island name=\"order-chart\" props={chart} />\
            {/if}{/each}";
        let data = json!({ "charts": [{ "title": "a", "points": [1] }] });

        assert_eq!(
            sent(source, &[], data),
            json!({ "charts": [{ "title": "a", "points": null }] })
        );
    }

    #[test]
    fn a_list_looped_over_outside_a_gate_keeps_its_length() {
        let source = "<p>{data.order.id}</p>{#each data.lines as line}{line.id}{/each}\
            {#each data.tags as tag}<i></i>{/each}{#if can(\"order.order.cost.read\")}\
            {data.order.cost}{#each data.lines as line}{line.cost}{/each}\
            {#each data.tags as tag}{tag}{/each}{/if}";
        let data = json!({
            "order": { "id": 1, "cost": 2 },
            "lines": { "items": [{ "id": 3, "cost": 4 }], "page": 1 },
            "tags": ["a", "b"],
        });

        let expected = json!({
            "order": { "id": 1, "cost": null },
            "lines": { "items": [{ "id": 3, "cost": null }], "page": null },
            "tags": [null, null],
        });
        assert_eq!(sent(source, &[], data), expected);
    }

    #[cfg(feature = "analysis")]
    #[test]
    fn names_the_paths_a_gate_may_withhold() {
        use crate::rendering::Withholdable;
        use crate::slots::Step;

        let source = "<p>{data.name}</p>{#if data.discount}<p>off</p>{/if}\
            {#each data.charts as chart}<p>{chart.title}</p>\
            {#if can(\"order.order.cost.read\")}{chart}{data.discount}{data.name}{/if}{/each}\
            {#if can(\"order.order.cost.read\")}{#each data.tags as tag}{tag}{/each}{/if}";
        let template = Template::parse("screen", source).unwrap();
        let field = |name: &str| Step::Field(name.to_owned());
        let named = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
        let path = |steps: Vec<Step>, nullable, tested, names: &[&str]| Withholdable {
            steps,
            nullable,
            tested,
            named_fields: named(names),
        };

        let expected = [
            path(
                Vec::new(),
                false,
                false,
                &["name", "discount", "charts", "tags"],
            ),
            path(vec![field("discount")], true, true, &[]),
            path(vec![field("charts")], false, false, &["items"]),
            path(vec![field("charts"), Step::Item], false, false, &["title"]),
            path(vec![field("tags")], true, false, &[]),
            path(vec![field("tags"), Step::Item], true, false, &[]),
        ];
        assert_eq!(super::withholdable(&template), expected);
    }

    #[test]
    fn a_record_read_whole_inside_a_gate_beside_its_looped_items_sends_only_the_rows_shown() {
        let source = "{#if can(\"order.order.cost.read\")}<p>{data.order}</p>{/if}\
            {#each data.order.items as line}<p>{line.id}</p>{/each}";
        let data =
            || json!({ "order": { "id": 1, "note": "n", "items": [{ "id": 2, "cost": 3 }] } });

        assert_eq!(
            sent(source, &[], data()),
            json!({ "order": { "id": null, "note": null, "items": [{ "id": 2, "cost": null }] } })
        );
        assert_eq!(sent(source, &[COST], data()), data());
    }

    #[test]
    fn the_data_read_whole_inside_a_gate_beside_its_looped_items_sends_only_the_rows_shown() {
        let source = "{#if can(\"order.order.cost.read\")}<p>{data}</p>{/if}\
            {#each data.items as line}<p>{line.id}</p>{/each}";
        let data = json!({ "name": "a", "items": [{ "id": 2, "cost": 3 }] });

        assert_eq!(
            sent(source, &[], data),
            json!({ "name": null, "items": [{ "id": 2, "cost": null }] })
        );
    }

    #[test]
    fn a_list_looped_over_only_inside_a_gate_keeps_no_length() {
        let source = "<p>{data.order.id}</p>{#if can(\"order.order.cost.read\")}\
            {#each data.order.lines as line}<i></i>{/each}\
            {#each data.order.items as line}<i></i>{/each}{/if}";
        let data = json!({ "order": { "id": 1, "lines": [1, 2], "items": [3, 4] } });

        assert_eq!(
            sent(source, &[], data),
            json!({ "order": { "id": 1, "lines": null, "items": null } })
        );
    }

    #[test]
    fn a_gated_read_of_a_pages_items_is_a_read_of_the_rows_a_table_shows() {
        let source = "<Table id=\"all\" of={data.orders} columns=\"id\" />\
            {#if can(\"order.order.cost.read\")}<p>{data.orders.items}</p>{/if}";
        let data = json!({ "orders": { "items": [{ "id": 1, "cost": 2 }], "page": 1 } });

        assert_eq!(
            sent(source, &[], data),
            json!({ "orders": { "items": [{ "id": 1, "cost": null }], "page": 1 } })
        );
    }

    #[test]
    fn a_page_looped_over_without_a_table_keeps_no_page_keys() {
        let source = "{#each data.lines as line}{line.id}{/each}\
            {#if can(\"order.order.cost.read\")}{#each data.lines as line}{line.cost}{/each}{/if}";
        let data = json!({ "lines": { "items": [{ "id": 3, "cost": 4 }], "page": 1 } });

        assert_eq!(
            sent(source, &[], data),
            json!({ "lines": { "items": [{ "id": 3, "cost": null }], "page": null } })
        );
    }

    #[test]
    fn a_page_key_a_gated_part_binds_is_sent_where_the_table_is_shown() {
        let source = "<Table id=\"all\" of={data.orders} columns=\"id\" />\
            {#if can(\"order.order.cost.read\")}<p>{data.orders.page} {data.orders.has_next}</p>\
            {/if}";
        let data = json!({ "orders": { "items": [{ "id": 1 }], "page": 2, "has_next": true } });

        assert_eq!(sent(source, &[], data.clone()), data);
    }

    #[test]
    fn a_table_of_a_pages_items_sends_no_page_keys() {
        let source = "{#if can(\"order.order.cost.read\")}\
            <Table id=\"all\" of={data.orders} columns=\"id\" />{/if}\
            <Table id=\"rows\" of={data.orders.items} columns=\"id\" />";
        let data = || {
            json!({ "orders": {
                "items": [{ "id": 1 }], "page": 1, "controls": {}, "filter": {}, "errors": {},
            } })
        };

        assert_eq!(
            sent(source, &[], data()),
            json!({ "orders": {
                "items": [{ "id": 1 }], "page": null, "controls": null, "filter": null,
                "errors": null,
            } })
        );
        assert_eq!(sent(source, &[COST], data()), data());
    }

    #[test]
    fn a_grandchild_record_under_a_failed_listing_read_is_withheld() {
        let source = "{#each data.orders as order}{order.id}{/each}\
            {#if can(\"order.order.cost.read\")}\
            <Table id=\"all\" of={data.orders} columns=\"id owner\" />{/if}";
        let data = || {
            json!({ "orders": {
                "items": [{ "id": 1, "owner": { "name": "a", "address": { "city": "c" } } }],
                "page": 1,
            } })
        };

        assert_eq!(
            sent(source, &[], data()),
            json!({ "orders": { "items": [{ "id": 1, "owner": null }], "page": null } })
        );
        assert_eq!(sent(source, &[COST], data()), data());
    }

    #[test]
    fn nothing_is_withheld_where_nothing_is_gated() {
        let template = Template::parse("screen", "<p>{data.name}</p>").unwrap();
        let viewer = Viewer { permissions: &[] };

        assert!(
            Rendering::new(&template)
                .gates(&viewer)
                .withheld()
                .is_empty()
        );
    }
}
