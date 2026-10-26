use std::fmt::Write as _;

use sofie_model::{Aggregate, EnumDeclaration, FieldDeclaration, Module, TypeName};

use super::markdown::{code, codes, mermaid, table};

pub(super) fn data_model(module: &Module) -> String {
    let aggregates = &module.code.aggregates;
    if aggregates.is_empty() {
        return String::new();
    }
    let mut text = String::from("## Data model\n\n");
    text.push_str(&mermaid("erDiagram", &entity_lines(module)));
    for aggregate in aggregates {
        text.push('\n');
        text.push_str(&aggregate_section(aggregate));
    }
    text.push('\n');
    text
}

fn entity_lines(module: &Module) -> Vec<String> {
    let mut lines = Vec::new();
    for aggregate in &module.code.aggregates {
        lines.push(format!("{} {{", aggregate.name));
        lines.push("    Uuid id PK".to_owned());
        for field in &aggregate.fields {
            lines.push(format!("    {}", attribute(aggregate, field)));
        }
        lines.push("}".to_owned());
    }
    for aggregate in &module.code.aggregates {
        for field in &aggregate.fields {
            if let Some(target) = field.ty.record() {
                let own = module
                    .code
                    .aggregates
                    .iter()
                    .any(|other| other.name == target);
                // A record of another module is joined by id, never by foreign key.
                let link = if own { "--" } else { ".." };
                lines.push(format!(
                    "{} }}o{link}|| {target} : {}",
                    aggregate.name, field.name
                ));
            }
        }
        for child in &aggregate.children {
            lines.push(format!(
                "{} ||--o{{ {} : {}",
                aggregate.name,
                diagram_type(child_type(child)),
                child.name
            ));
        }
    }
    lines
}

fn child_type(field: &FieldDeclaration) -> &TypeName {
    field.ty.arguments.first().unwrap_or(&field.ty)
}

fn attribute(aggregate: &Aggregate, field: &FieldDeclaration) -> String {
    let mut line = format!("{} {}", diagram_type(&field.ty), field.name);
    if field.ty.record().is_some() {
        line.push_str(" FK");
    }
    let mut notes = field.constraints.clone();
    if aggregate.owner.as_ref() == Some(&field.name) {
        notes.insert(0, "owner".to_owned());
    }
    if !notes.is_empty() {
        let _ = write!(line, " \"{}\"", notes.join(", "));
    }
    line
}

/// Mermaid's attribute types take letters, digits, `_`, `-`, `()` and `[]`.
fn diagram_type(ty: &TypeName) -> String {
    let arguments: Vec<String> = ty.arguments.iter().map(diagram_type).collect();
    let name = match ty.name.as_str() {
        "&" | "(…)" | "()" | "_" | "dyn" => "Value",
        name => name,
    };
    if arguments.is_empty() {
        return name.to_owned();
    }
    format!("{name}({})", arguments.join("-"))
}

fn aggregate_section(aggregate: &Aggregate) -> String {
    let mut text = format!("### {}\n\n", aggregate.name);
    if let Some(doc) = &aggregate.doc {
        let _ = writeln!(text, "{doc}\n");
    }
    let behaviors: Vec<&str> = aggregate.all_behaviors().collect();
    let _ = writeln!(text, "- Behaviors: {}", codes(&behaviors));
    let _ = writeln!(
        text,
        "- Owner: {}",
        aggregate.owner.as_deref().map_or("—".to_owned(), code)
    );
    let _ = writeln!(
        text,
        "- Exposed to other modules: {}",
        codes(&aggregate.exposed)
    );
    if let Some(mirror) = &aggregate.mirror {
        let _ = writeln!(text, "- Mirrors: {}", code(&mirror.join("::")));
    }
    if !aggregate.children.is_empty() {
        let children: Vec<String> = aggregate
            .children
            .iter()
            .map(|child| {
                format!(
                    "{} ({})",
                    code(&child.name),
                    code(&child_type(child).to_string())
                )
            })
            .collect();
        let _ = writeln!(text, "- Children: {}", children.join(", "));
    }
    let _ = writeln!(text, "- Columns: {}\n", codes(&aggregate.columns()));
    let rows: Vec<[String; 3]> = aggregate
        .fields
        .iter()
        .map(|field| {
            [
                code(&field.name),
                code(&field.ty.to_string()),
                codes(&field.constraints),
            ]
        })
        .collect();
    text.push_str(&table(["Field", "Type", "Constraints"], &rows));
    text
}

pub(super) fn state_machines(module: &Module) -> String {
    let machines: Vec<&EnumDeclaration> = module
        .code
        .enums
        .iter()
        .filter(|declared| declared.has_transitions())
        .collect();
    if machines.is_empty() {
        return String::new();
    }
    let mut text = String::from("## State machines\n");
    for machine in machines {
        let _ = write!(text, "\n### {}\n\n", machine.name);
        text.push_str(&mermaid("stateDiagram-v2", &transition_lines(machine)));
    }
    text.push('\n');
    text
}

fn transition_lines(machine: &EnumDeclaration) -> Vec<String> {
    machine
        .variants
        .iter()
        .flat_map(|variant| {
            if variant.to.is_empty() {
                return vec![format!("{} --> [*]", variant.name)];
            }
            variant
                .to
                .iter()
                .map(|target| format!("{} --> {target}", variant.name))
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use sofie_model::testing::{aggregate, enumeration, field, module};

    use super::{data_model, state_machines};

    #[test]
    fn draws_entities_with_references_and_a_state_machine() {
        let mut order = module("order", "shop/modules/order");
        let mut placed = aggregate(
            "Order",
            vec![
                field("account", "AccountId", 2),
                field("status", "OrderStatus", 3),
            ],
            "shop/modules/order/src/order.rs",
        );
        placed.owner = Some("account".to_owned());
        let entry = aggregate(
            "AuditEntry",
            vec![field("order", "Id<Order>", 2)],
            "shop/modules/order/src/audit.rs",
        );
        order.code.aggregates = vec![placed, entry];
        order.code.enums = vec![
            enumeration(
                "OrderStatus",
                &[
                    ("Pending", &["Paid", "Cancelled"]),
                    ("Paid", &[]),
                    ("Cancelled", &[]),
                ],
            ),
            enumeration("AuditAction", &[("Placed", &[])]),
        ];

        let model = data_model(&order);
        let machines = state_machines(&order);

        assert!(
            model.contains(
                "erDiagram\n    Order {\n        Uuid id PK\n        AccountId account FK \"owner\"\n        \
                 OrderStatus status\n    }\n    AuditEntry {\n        Uuid id PK\n        Id(Order) order FK\n    }\n    \
                 Order }o..|| Account : account\n    AuditEntry }o--|| Order : order\n```"
            ),
            "{model}"
        );
        assert!(model.contains("| `account` | `AccountId` | — |"), "{model}");
        assert_eq!(
            machines,
            "## State machines\n\n### OrderStatus\n\n```mermaid\nstateDiagram-v2\n    Pending --> Paid\n    \
             Pending --> Cancelled\n    Paid --> [*]\n    Cancelled --> [*]\n```\n\n"
        );
    }
}
