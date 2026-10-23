use std::fmt;
use std::ops::Add;

use crate::lines::LineKind;
use crate::workspace::{CrateSource, SourceFile};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommentDensity {
    pub code_lines: usize,
    pub comment_lines: usize,
}

impl CommentDensity {
    #[must_use]
    pub fn of_file(file: &SourceFile) -> Self {
        let count = |wanted: LineKind| file.lines.iter().filter(|kind| **kind == wanted).count();
        Self {
            code_lines: count(LineKind::Code),
            comment_lines: count(LineKind::Comment),
        }
    }

    #[must_use]
    pub fn of_crate(crate_source: &CrateSource) -> Self {
        crate_source
            .files
            .iter()
            .map(Self::of_file)
            .fold(Self::default(), Add::add)
    }

    #[must_use]
    pub fn within_limit(self) -> bool {
        self.comment_lines <= crate::structure::FREE_COMMENT_LINES
            || self.ratio() <= crate::structure::MAX_COMMENT_RATIO
    }

    #[must_use]
    pub fn ratio(self) -> f64 {
        if self.code_lines == 0 {
            return 0.0;
        }
        crate::count_as_f64(self.comment_lines) / crate::count_as_f64(self.code_lines)
    }
}

impl Add for CommentDensity {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            code_lines: self.code_lines + other.code_lines,
            comment_lines: self.comment_lines + other.comment_lines,
        }
    }
}

impl fmt::Display for CommentDensity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "ratio {:.3} ({} comment / {} code lines)",
            self.ratio(),
            self.comment_lines,
            self.code_lines
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::CommentDensity;
    use crate::workspace::SourceFile;

    fn density(text: &str) -> CommentDensity {
        CommentDensity::of_file(&SourceFile::parse(PathBuf::from("lib.rs"), text).unwrap())
    }

    #[test]
    fn counts_code_and_comment_lines() {
        let density = density("// why\nfn a() {}\n\nfn b() {} // trailing\n");
        assert_eq!(
            density,
            CommentDensity {
                code_lines: 2,
                comment_lines: 1
            }
        );
        assert!((density.ratio() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn source_without_code_has_zero_ratio() {
        assert!(density("// only a comment").ratio().abs() < f64::EPSILON);
    }
}
