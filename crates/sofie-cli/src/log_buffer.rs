use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::sync::{Arc, Mutex, MutexGuard};

use sofie_core::unpoisoned;

pub(crate) const CAPACITY: usize = 10_000;
/// Per kind, so a backlog of jobs drained at start keeps no request's tree out.
pub(crate) const TREES_PER_KIND: usize = 500;
const DROPPED_TRACE_IDS: usize = 20_000;
const TREE_MARKER: &str = " sofie.dev: trace_id=\"";
const REQUEST_ROOT: &str = "request ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stream {
    Out,
    Error,
}

struct Line {
    number: u64,
    text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TreeKind {
    Request,
    Background,
}

#[derive(Default)]
struct Contents {
    lines: VecDeque<Line>,
    requests: VecDeque<Tree>,
    background: VecDeque<Tree>,
    /// The tree whose indented stdout lines are still arriving.
    growing: Option<TreeKind>,
    dropped_trace_ids: VecDeque<String>,
}

/// The development app's stdout and stderr, numbered from 1, and the span
/// trees in them, kept apart so a flood of lines leaves the trees; the
/// oldest of each go first once it is full.
#[derive(Default)]
pub(crate) struct LogBuffer {
    contents: Mutex<Contents>,
}

#[cfg_attr(
    not(any(feature = "mcp", test)),
    expect(dead_code, reason = "`sofie mcp` pages through the log")
)]
pub(crate) struct Page {
    pub(crate) lines: Vec<String>,
    pub(crate) next: u64,
    pub(crate) dropped: bool,
}

#[derive(Clone)]
pub(crate) struct Tree {
    pub(crate) trace_id: String,
    pub(crate) lines: Vec<String>,
}

pub(crate) struct TraceTrees {
    pub(crate) trees: Vec<Tree>,
    #[cfg_attr(
        not(any(feature = "mcp", test)),
        expect(dead_code, reason = "`sofie mcp` says a tree was dropped")
    )]
    pub(crate) dropped: bool,
}

#[cfg_attr(
    not(any(feature = "mcp", test)),
    expect(dead_code, reason = "`sofie mcp` lists the trees")
)]
pub(crate) struct RecentTrees {
    pub(crate) requests: Vec<Tree>,
    pub(crate) background: Vec<Tree>,
}

impl LogBuffer {
    pub(crate) fn follow(
        self: &Arc<Self>,
        stdout: Option<impl Read + Send + 'static>,
        stderr: Option<impl Read + Send + 'static>,
    ) {
        if let Some(stdout) = stdout {
            self.read_lines(Stream::Out, stdout);
        }
        if let Some(stderr) = stderr {
            self.read_lines(Stream::Error, stderr);
        }
    }

    fn read_lines(self: &Arc<Self>, stream: Stream, reader: impl Read + Send + 'static) {
        let buffer = Arc::clone(self);
        std::thread::spawn(move || {
            for line in BufReader::new(reader).split(b'\n') {
                match line {
                    Ok(bytes) => buffer.push(stream, &String::from_utf8_lossy(&bytes)),
                    Err(error) => {
                        buffer.push(
                            Stream::Error,
                            &format!("reading the app's output failed: {error}"),
                        );
                        return;
                    }
                }
            }
        });
    }

    fn contents(&self) -> MutexGuard<'_, Contents> {
        unpoisoned::lock(&self.contents)
    }

    fn push(&self, stream: Stream, text: &str) {
        let text = text.trim_end_matches('\r');
        let mut contents = self.contents();
        if stream == Stream::Out {
            contents.grow_trees(text);
        }
        let number = contents.lines.back().map_or(1, |last| last.number + 1);
        if contents.lines.len() == CAPACITY {
            contents.lines.pop_front();
        }
        contents.lines.push_back(Line {
            number,
            text: text.to_owned(),
        });
    }

    pub(crate) fn after(&self, cursor: u64, filter: Option<&str>, limit: usize) -> Page {
        let contents = self.contents();
        let lines = &contents.lines;
        let dropped = lines
            .front()
            .is_some_and(|first| first.number > cursor.saturating_add(1));
        let mut next = cursor.max(lines.back().map_or(0, |last| last.number));
        let mut shown = Vec::new();
        for line in lines.iter().filter(|line| line.number > cursor) {
            if shown.len() == limit {
                next = line.number - 1;
                break;
            }
            if filter.is_none_or(|filter| line.text.contains(filter)) {
                shown.push(format!("{} {}", line.number, line.text));
            }
        }
        Page {
            lines: shown,
            next,
            dropped,
        }
    }

    pub(crate) fn cursor(&self) -> u64 {
        self.contents().lines.back().map_or(0, |last| last.number)
    }

    pub(crate) fn tail(&self, count: usize) -> Vec<String> {
        let contents = self.contents();
        let skipped = contents.lines.len().saturating_sub(count);
        contents
            .lines
            .iter()
            .skip(skipped)
            .map(|line| line.text.clone())
            .collect()
    }

    pub(crate) fn trees(&self, trace_id: &str) -> TraceTrees {
        let contents = self.contents();
        let trees = contents
            .requests
            .iter()
            .chain(&contents.background)
            .filter(|tree| tree.trace_id == trace_id)
            .cloned()
            .collect();
        let dropped = contents.dropped_trace_ids.iter().any(|id| id == trace_id);
        TraceTrees { trees, dropped }
    }

    #[cfg_attr(
        not(any(feature = "mcp", test)),
        expect(dead_code, reason = "`sofie mcp` lists the trees")
    )]
    pub(crate) fn recent(&self, count: usize) -> RecentTrees {
        let contents = self.contents();
        let last = |trees: &VecDeque<Tree>| {
            trees
                .iter()
                .skip(trees.len().saturating_sub(count))
                .cloned()
                .collect()
        };
        RecentTrees {
            requests: last(&contents.requests),
            background: last(&contents.background),
        }
    }
}

impl Contents {
    /// A tree's lines are contiguous on stdout; stderr may interleave.
    fn grow_trees(&mut self, text: &str) {
        if let Some((trace_id, first)) = tree_header(text) {
            let kind = if first.starts_with(REQUEST_ROOT) {
                TreeKind::Request
            } else {
                TreeKind::Background
            };
            self.keep(
                kind,
                Tree {
                    trace_id: trace_id.to_owned(),
                    lines: vec![first.to_owned()],
                },
            );
            return;
        }
        let growing = self.growing.filter(|_| text.starts_with(' '));
        self.growing = growing;
        let tree = match growing {
            Some(TreeKind::Request) => self.requests.back_mut(),
            Some(TreeKind::Background) => self.background.back_mut(),
            None => None,
        };
        if let Some(tree) = tree {
            tree.lines.push(text.to_owned());
        }
    }

    fn keep(&mut self, kind: TreeKind, tree: Tree) {
        let trees = match kind {
            TreeKind::Request => &mut self.requests,
            TreeKind::Background => &mut self.background,
        };
        if trees.len() == TREES_PER_KIND
            && let Some(dropped) = trees.pop_front()
        {
            if self.dropped_trace_ids.len() == DROPPED_TRACE_IDS {
                self.dropped_trace_ids.pop_front();
            }
            self.dropped_trace_ids.push_back(dropped.trace_id);
        }
        trees.push_back(tree);
        self.growing = Some(kind);
    }
}

fn tree_header(text: &str) -> Option<(&str, &str)> {
    let (_, marked) = text.split_once(TREE_MARKER)?;
    let (trace_id, first) = marked.split_once('"')?;
    Some((trace_id, first.trim_start()))
}

#[cfg(test)]
mod tests {
    use super::{CAPACITY, LogBuffer, Stream, TREES_PER_KIND};

    const ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

    fn header(trace_id: &str, rest: &str) -> String {
        format!("2026-10-13T09:00:00Z  INFO sofie.dev: trace_id=\"{trace_id}\" {rest}")
    }

    #[test]
    fn a_full_buffer_drops_its_oldest_lines_and_a_cursor_reads_on_from_where_it_was() {
        let buffer = LogBuffer::default();
        for number in 1..=CAPACITY + 5 {
            buffer.push(Stream::Out, &format!("line {number}"));
        }

        let first = buffer.after(0, None, 3);
        assert!(first.dropped);
        assert_eq!(first.lines, ["6 line 6", "7 line 7", "8 line 8"]);
        assert_eq!(first.next, 8);

        let filtered = buffer.after(first.next, Some("line 200"), 3);
        assert!(!filtered.dropped);
        assert_eq!(
            filtered.lines,
            ["200 line 200", "2000 line 2000", "2001 line 2001"]
        );
        assert_eq!(filtered.next, 2001);

        let end = buffer.after(CAPACITY as u64 + 5, None, 10);
        assert_eq!(end.lines, Vec::<String>::new());
        assert_eq!(end.next, CAPACITY as u64 + 5);
    }

    /// `after` comes from an MCP client as any number.
    #[test]
    fn a_cursor_past_every_line_shows_none_and_drops_none() {
        let buffer = LogBuffer::default();
        buffer.push(Stream::Out, "line 1");

        let page = buffer.after(u64::MAX, None, 3);

        assert_eq!(page.lines, [] as [String; 0]);
        assert!(!page.dropped);
        assert_eq!(page.next, u64::MAX);
    }

    #[test]
    fn a_trace_finds_each_of_its_trees_with_their_indented_lines() {
        let buffer = LogBuffer::default();
        buffer.push(Stream::Out, &header(ID, "request GET / 3.10 ms"));
        buffer.push(Stream::Error, "  a warning between");
        buffer.push(Stream::Out, "  handler 2.00 ms");
        buffer.push(Stream::Out, "2026-10-13T09:00:01Z  INFO sofie_http: other");
        buffer.push(Stream::Out, "  not part of a tree");
        buffer.push(
            Stream::Out,
            &header("00000000000000000000000000000001", "job 1.00 ms"),
        );
        buffer.push(Stream::Out, &header(ID, "job send_receipt 4.00 ms"));

        let found = buffer.trees(ID);

        assert!(!found.dropped);
        assert_eq!(found.trees.len(), 2);
        assert_eq!(
            found.trees[0].lines,
            ["request GET / 3.10 ms", "  handler 2.00 ms"]
        );
        assert_eq!(found.trees[1].lines, ["job send_receipt 4.00 ms"]);
        let recent = buffer.recent(10);
        assert_eq!(recent.requests.len(), 1);
        assert_eq!(recent.background.len(), 2);
    }

    #[test]
    fn a_flood_of_lines_and_job_trees_keeps_a_request_s_tree_and_names_a_dropped_one() {
        let buffer = LogBuffer::default();
        buffer.push(
            Stream::Out,
            &header("00000000000000000000000000000001", "job audit 1.00 ms"),
        );
        buffer.push(Stream::Out, &header(ID, "request GET /orders 2.00 ms"));
        for number in 0..CAPACITY.max(TREES_PER_KIND) {
            let job = format!("{:032x}", number + 2);
            buffer.push(Stream::Out, &header(&job, "listener audit 1.00 ms"));
        }

        let request = buffer.trees(ID);
        assert_eq!(request.trees.len(), 1);
        assert_eq!(
            buffer.after(0, Some("GET /orders"), 10).lines,
            Vec::<String>::new()
        );

        let job = buffer.trees("00000000000000000000000000000001");
        assert!(job.trees.is_empty());
        assert!(job.dropped);
        assert!(!buffer.trees("ffffffffffffffffffffffffffffffff").dropped);
    }
}
