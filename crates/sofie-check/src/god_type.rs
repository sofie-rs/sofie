use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

use proc_macro2::{LineColumn, TokenStream, TokenTree};
use sofie_model::syntax::use_paths;
use syn::{ImplItem, Item, ItemImpl, Type, Visibility};

use crate::count_as_f64;
use crate::lines::LineKind;
use crate::workspace::{CrateSource, SourceFile};

/// Divisors, not cutoffs: the score reads as "how many times over", summed across axes.
const CODE_LINES_THRESHOLD: f64 = 300.0;
const METHODS_THRESHOLD: f64 = 15.0;
const DEPENDENCIES_THRESHOLD: f64 = 12.0;
const PUBLIC_METHODS_THRESHOLD: f64 = 10.0;

const INTERNAL_PATH_ROOTS: [&str; 2] = ["crate", "super"];
const WORKSPACE_CRATE_PREFIX: &str = "sofie_";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TypeMetrics {
    pub code_lines: usize,
    pub methods: usize,
    pub public_methods: usize,
    pub dependencies: usize,
}

impl TypeMetrics {
    #[must_use]
    pub fn score(self) -> f64 {
        count_as_f64(self.code_lines) / CODE_LINES_THRESHOLD
            + count_as_f64(self.methods) / METHODS_THRESHOLD
            + count_as_f64(self.dependencies) / DEPENDENCIES_THRESHOLD
            + count_as_f64(self.public_methods) / PUBLIC_METHODS_THRESHOLD
    }
}

#[derive(Debug)]
pub struct GodType {
    pub type_name: String,
    pub crate_name: String,
    pub files: Vec<PathBuf>,
    pub metrics: TypeMetrics,
}

impl GodType {
    #[must_use]
    pub fn score(&self) -> f64 {
        self.metrics.score()
    }
}

impl fmt::Display for GodType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let files: Vec<String> = self
            .files
            .iter()
            .map(|file| file.display().to_string())
            .collect();
        write!(
            formatter,
            "{}::{} score {:.2} ({} code lines, {} methods, {} public, {} dependencies) in {}",
            self.crate_name,
            self.type_name,
            self.score(),
            self.metrics.code_lines,
            self.metrics.methods,
            self.metrics.public_methods,
            self.metrics.dependencies,
            files.join(", ")
        )
    }
}

#[must_use]
pub fn rank_god_types(crates: &[CrateSource]) -> Vec<GodType> {
    let mut ranking: Vec<GodType> = crates.iter().flat_map(crate_god_types).collect();
    ranking.sort_by(|left, right| right.score().total_cmp(&left.score()));
    ranking
}

#[derive(Default)]
struct TypeAccumulator {
    metrics: TypeMetrics,
    dependencies: BTreeSet<String>,
    files: BTreeSet<PathBuf>,
}

fn crate_god_types(crate_source: &CrateSource) -> Vec<GodType> {
    let mut types: BTreeMap<String, TypeAccumulator> = BTreeMap::new();
    for file in &crate_source.files {
        add_file_impls(file, &mut types);
    }
    types
        .into_iter()
        .map(|(type_name, accumulator)| GodType {
            type_name,
            crate_name: crate_source.name.clone(),
            metrics: TypeMetrics {
                dependencies: accumulator.dependencies.len(),
                ..accumulator.metrics
            },
            files: accumulator.files.into_iter().collect(),
        })
        .collect()
}

fn add_file_impls(file: &SourceFile, types: &mut BTreeMap<String, TypeAccumulator>) {
    let mut impls = Vec::new();
    collect_inherent_impls(&file.syntax.items, &mut impls);
    let mut imports = Vec::new();
    collect_internal_imports(&file.syntax.items, &mut imports);
    let identifiers = located_identifiers(file.tokens.clone());
    for item_impl in impls {
        let Some(type_name) = self_type_name(&item_impl.self_ty) else {
            continue;
        };
        let accumulator = types.entry(type_name).or_default();
        let metrics = &mut accumulator.metrics;
        metrics.code_lines += impl_code_lines(item_impl, &file.lines);
        metrics.methods += item_impl
            .items
            .iter()
            .filter(|item| matches!(item, ImplItem::Fn(_)))
            .count();
        metrics.public_methods += public_method_count(item_impl);
        accumulator
            .dependencies
            .extend(imports_used_by(item_impl, &imports, &identifiers));
        accumulator.files.insert(file.path.clone());
    }
}

fn collect_inherent_impls<'syntax>(items: &'syntax [Item], found: &mut Vec<&'syntax ItemImpl>) {
    for item in items {
        match item {
            Item::Impl(item_impl) if item_impl.trait_.is_none() => found.push(item_impl),
            Item::Mod(module) => {
                if let Some((_, module_items)) = &module.content {
                    collect_inherent_impls(module_items, found);
                }
            }
            _ => {}
        }
    }
}

fn self_type_name(self_type: &Type) -> Option<String> {
    let Type::Path(type_path) = self_type else {
        return None;
    };
    type_path
        .path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

fn impl_code_lines(item_impl: &ItemImpl, line_kinds: &[LineKind]) -> usize {
    let first_line = item_impl.impl_token.span.start().line;
    let last_line = item_impl.brace_token.span.close().end().line;
    line_kinds[first_line - 1..last_line]
        .iter()
        .filter(|kind| **kind == LineKind::Code)
        .count()
}

fn public_method_count(item_impl: &ItemImpl) -> usize {
    item_impl
        .items
        .iter()
        .filter(|item| matches!(item, ImplItem::Fn(method) if matches!(method.vis, Visibility::Public(_))))
        .count()
}

struct Import {
    path: String,
    /// `None` for a glob, whose names cannot be told apart, so it always counts.
    binding: Option<String>,
}

fn collect_internal_imports(items: &[Item], found: &mut Vec<Import>) {
    for item in items {
        match item {
            Item::Use(item_use) => found.extend(
                use_paths(&item_use.tree)
                    .into_iter()
                    .filter(|used| is_internal_path(&used.segments))
                    .map(|used| {
                        let glob = used.binding.is_none().then(|| "*".to_owned());
                        Import {
                            path: used
                                .segments
                                .into_iter()
                                .chain(glob)
                                .collect::<Vec<_>>()
                                .join("::"),
                            binding: used.binding,
                        }
                    }),
            ),
            Item::Mod(module) => {
                if let Some((_, module_items)) = &module.content {
                    collect_internal_imports(module_items, found);
                }
            }
            _ => {}
        }
    }
}

/// A file's imports serve all of its items, so only the imported names an impl mentions are its coupling.
fn imports_used_by(
    item_impl: &ItemImpl,
    imports: &[Import],
    identifiers: &[(LineColumn, String)],
) -> Vec<String> {
    let start = item_impl.impl_token.span.start();
    let end = item_impl.brace_token.span.close().end();
    let mentioned: BTreeSet<&str> = identifiers
        .iter()
        .filter(|(position, _)| (start..end).contains(position))
        .map(|(_, identifier)| identifier.as_str())
        .collect();
    imports
        .iter()
        .filter(|import| {
            import
                .binding
                .as_deref()
                .is_none_or(|binding| mentioned.contains(binding))
        })
        .map(|import| import.path.clone())
        .collect()
}

fn located_identifiers(tokens: TokenStream) -> Vec<(LineColumn, String)> {
    let mut found = Vec::new();
    for token in tokens {
        match token {
            TokenTree::Ident(identifier) => {
                found.push((identifier.span().start(), identifier.to_string()));
            }
            TokenTree::Group(group) => found.extend(located_identifiers(group.stream())),
            TokenTree::Punct(_) | TokenTree::Literal(_) => {}
        }
    }
    found
}

fn is_internal_path(segments: &[String]) -> bool {
    segments.first().is_some_and(|root| {
        INTERNAL_PATH_ROOTS.contains(&root.as_str()) || root.starts_with(WORKSPACE_CRATE_PREFIX)
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{GodType, TypeMetrics, rank_god_types};
    use crate::workspace::{CrateSource, SourceFile};

    fn crate_with(files: &[(&str, &str)]) -> CrateSource {
        CrateSource {
            name: "sofie-sample".to_owned(),
            files: files
                .iter()
                .map(|(path, text)| {
                    SourceFile::parse(PathBuf::from(path), text).expect("sample sources parse")
                })
                .collect(),
        }
    }

    fn rank(files: &[(&str, &str)]) -> Vec<GodType> {
        rank_god_types(&[crate_with(files)])
    }

    #[test]
    fn score_follows_the_weighted_formula() {
        let metrics = TypeMetrics {
            code_lines: 300,
            methods: 15,
            public_methods: 10,
            dependencies: 12,
        };
        assert!((metrics.score() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn measures_methods_public_methods_and_code_lines() {
        let source = "struct Widget;\nimpl Widget {\n    // why\n    pub fn a() {}\n\n    fn b() {}\n    pub(crate) fn c() {}\n    const D: u8 = 1;\n}\n";
        let ranking = rank(&[("lib.rs", source)]);
        assert_eq!(ranking.len(), 1);
        assert_eq!(ranking[0].type_name, "Widget");
        let expected = TypeMetrics {
            code_lines: 6,
            methods: 3,
            public_methods: 1,
            dependencies: 0,
        };
        assert_eq!(ranking[0].metrics, expected);
    }

    #[test]
    fn merges_impls_of_one_type_across_files_and_ignores_trait_impls() {
        let first = "use crate::a::B;\nimpl Widget { fn a() {} }\n";
        let second = "use super::{c::D, e::*};\nimpl crate::Widget { fn b() {} }\nimpl Clone for Widget { fn clone(&self) -> Self { todo!() } }\n";
        let ranking = rank(&[("a.rs", first), ("b.rs", second)]);
        assert_eq!(ranking.len(), 1);
        assert_eq!(ranking[0].metrics.methods, 2);
        assert_eq!(ranking[0].metrics.code_lines, 2);
        assert_eq!(
            ranking[0].files,
            [PathBuf::from("a.rs"), PathBuf::from("b.rs")]
        );
    }

    #[test]
    fn counts_the_internal_imports_the_impls_mention_and_every_glob() {
        let source = "use std::fmt;\nuse sofie_core::Thing;\nuse crate::{a::{self, B}, a::B as C, unused::D, e::*};\nmod inner { use super::x::Y; }\nimpl Widget { fn f(_: fmt::Debug, _: Thing, _: a::Z, _: B, _: C, _: Y) {} }\nimpl Other {}\n";
        let ranking = rank(&[("lib.rs", source)]);
        let dependencies = |name: &str| {
            ranking
                .iter()
                .find(|god_type| god_type.type_name == name)
                .map(|god_type| god_type.metrics.dependencies)
        };
        assert_eq!(dependencies("Widget"), Some(5));
        assert_eq!(dependencies("Other"), Some(1));
    }

    #[test]
    fn finds_impls_inside_inline_modules_and_ranks_worst_first() {
        let source =
            "impl Small {}\nmod inner {\n    impl Large { pub fn a() {} pub fn b() {} }\n}\n";
        let ranking = rank(&[("lib.rs", source)]);
        let names: Vec<&str> = ranking
            .iter()
            .map(|god_type| god_type.type_name.as_str())
            .collect();
        assert_eq!(names, ["Large", "Small"]);
    }
}
