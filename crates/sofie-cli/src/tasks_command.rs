use sofie_model::naming::task_name;
use sofie_model::{App, Module, Task};

use crate::routes_command::text_table;
use crate::sorted;

const HEADERS: [&str; 4] = ["NAME", "ARGUMENTS", "FUNCTION", "DESCRIPTION"];

#[must_use]
pub fn task_table(app: &App) -> String {
    let mut rows: Vec<[String; 4]> = sorted::modules(app)
        .into_iter()
        .flat_map(|module| module.tasks.iter().map(move |task| row(module, task)))
        .collect();
    rows.sort();
    text_table(HEADERS, &rows)
}

fn row(module: &Module, task: &Task) -> [String; 4] {
    let function = module.code.function(&task.function);
    [
        task_name(module, &task.function),
        task_arguments(module, task),
        format!("{}::{}", module.krate.crate_name, task.function.join("::")),
        function
            .and_then(|function| function.doc.clone())
            .unwrap_or_default(),
    ]
}

/// As the binary reads them: `--field value`, an optional one in brackets.
pub(crate) fn task_arguments(module: &Module, task: &Task) -> String {
    let Some(input) = module
        .code
        .function(&task.function)
        .and_then(|function| module.code.input_of(function))
    else {
        return String::new();
    };
    let fields: Vec<String> = input
        .fields
        .iter()
        .map(|field| match field.ty.arguments.as_slice() {
            [inner] if field.ty.name == "Option" => format!("[--{} {inner}]", field.name),
            _ => format!("--{} {}", field.name, field.ty),
        })
        .collect();
    fields.join(" ")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sofie_model::testing::{app, field, function, module, type_name};
    use sofie_model::{InputDeclaration, Task};

    use super::task_table;

    #[test]
    fn lists_each_task_with_the_arguments_its_input_reads() {
        let mut order = module("order", "shop/modules/order");
        let file = "shop/modules/order/src/tasks.rs";
        let mut backfill = function("tasks::backfill", file, 3);
        backfill.arguments = vec![type_name("Read"), type_name("Since")];
        backfill.doc = Some("Fills totals of orders placed before totals were kept.".to_owned());
        order.tasks = vec![Task {
            file: PathBuf::from(file),
            line: 3,
            function: backfill.path.clone(),
        }];
        order.code.functions = vec![backfill];
        order.code.inputs = vec![InputDeclaration {
            name: "Since".to_owned(),
            path: vec!["tasks".to_owned(), "Since".to_owned()],
            from: None,
            fields: vec![field("since", "Date", 1), field("limit", "Option<u32>", 1)],
            unconstrained: Vec::new(),
            file: PathBuf::from(file),
            line: 1,
        }];

        let table = task_table(&app("shop", vec![order]));

        let row: Vec<&str> = table
            .lines()
            .nth(1)
            .unwrap()
            .split("  ")
            .filter(|cell| !cell.is_empty())
            .map(str::trim)
            .collect();
        assert_eq!(
            row,
            [
                "order.backfill",
                "--since Date [--limit u32]",
                "order::tasks::backfill",
                "Fills totals of orders placed before totals were kept."
            ],
            "{table}"
        );
    }
}
