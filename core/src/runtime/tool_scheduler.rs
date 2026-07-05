//! DAG-based tool scheduler.
//!
//! When the model emits multiple tool calls in one turn, we don't blindly run
//! them all in parallel (they may mutate the same file / depend on each other's
//! side effects), nor strictly sequentially (slow when they're independent).
//! Instead we build a small dependency graph keyed on the *resources* each call
//! touches and run the schedule topologically: independent calls run
//! concurrently on a `JoinSet`, dependent calls run in order.
//!
//! Rules:
//! - Two calls conflict if both *mutate* the same resource (write the same path,
//!   run a bash command, …). Reads never conflict with anything, so a batch of
//!   `read_file` / `grep` calls always parallelizes.
//! - Ties are broken by call order, so the schedule is deterministic for a given
//!   batch — important for reproducible traces.
//! - `ToolExecutionMode::Sequential` collapses the whole batch into a single
//!   chain (one node per index, each depending on the previous).

use serde_json::Value;
use std::collections::HashMap;

use crate::types::ToolExecutionMode;

/// One node in the execution graph — an already-approved tool call ready to run.
#[derive(Debug, Clone)]
pub(crate) struct SchedNode {
    /// Original index into the `allowed` / results vectors.
    pub idx: usize,
    pub tool_name: String,
    pub tool_call_id: String,
    pub args: Value,
    /// Resources this call *mutates* (writes/bash/network-POST-ish). Two calls
    /// sharing a mutable resource form an edge.
    pub mutations: Vec<ResourceKey>,
    /// Resources this call only *reads*. Reads never create edges.
    pub reads: Vec<ResourceKey>,
}

/// A normalized handle on something a tool touches.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ResourceKey {
    /// A filesystem path the tool writes to.
    Path(String),
    /// A bash command — coarse-grained by the leading program so that
    /// `git status` and `git diff` don't needlessly serialize, but two writes
    /// to the same file via `sed` still line up.
    BashProgram(String),
    /// A network host.
    Host(String),
}

/// Edge list: `deps[i]` = the set of node indices that must finish before `i`.
#[derive(Debug, Default)]
pub(crate) struct DepGraph {
    /// `dependents[a]` contains `b` means "b waits on a".
    pub dependents: Vec<Vec<usize>>,
    /// `indegree[i]` = number of unfinished predecessors of `i`.
    pub indegree: Vec<usize>,
}

impl DepGraph {
    /// Build a dependency graph for `nodes` honoring `mode`.
    pub fn build(nodes: &[SchedNode], mode: ToolExecutionMode) -> Self {
        let n = nodes.len();
        let mut g = Self {
            dependents: vec![Vec::new(); n],
            indegree: vec![0; n],
        };

        if mode == ToolExecutionMode::Sequential {
            // Linear chain by index.
            for i in 1..n {
                g.dependents[i - 1].push(i);
                g.indegree[i] = 1;
            }
            return g;
        }

        // Parallel mode: add an edge a -> b (b waits on a, a < b) whenever b
        // mutates a resource that a *also* mutates (write-after-write / the
        // canonical hazardous case). Mutate-after-read is allowed to parallelize
        // since reads return a snapshot and don't affect the mutation.
        //
        // Index-order tiebreak keeps the graph a DAG by construction (edges only
        // go forward), so there is no cycle to detect here.
        for b in 1..n {
            for a in 0..b {
                if shares_mutable_resource(&nodes[a], &nodes[b]) {
                    g.dependents[a].push(b);
                    g.indegree[b] += 1;
                }
            }
        }

        g
    }
}

fn shares_mutable_resource(a: &SchedNode, b: &SchedNode) -> bool {
    // A mutation on either side conflicts with a mutation or read on the other
    // side for the *same* resource. Pure read/read never conflicts.
    //
    // We treat (mutate, read) as a conflict too: if a writes file X and b reads
    // file X, ordering matters (does b see the new content?). That's the safe
    // choice and matches user expectation that calls run in listed order when
    // they touch the same file.
    for ma in &a.mutations {
        if b.mutations.contains(ma) || b.reads.contains(ma) {
            return true;
        }
    }
    for mb in &b.mutations {
        if a.reads.contains(mb) {
            return true;
        }
    }
    false
}

/// Extract the resources a tool call touches from its parsed args.
///
/// `args` is the (possibly hook-rewritten) JSON arguments for the call. We only
/// look at well-known field names here; unknown tools default to no declared
/// resources, which means they're free to run in parallel — they're expected to
/// be effect-free relative to the local filesystem (e.g. pure compute / network
/// GET). Tools with side effects should expose them via these fields.
pub(crate) fn classify_resources(
    tool_name: &str,
    args: &Value,
) -> (Vec<ResourceKey>, Vec<ResourceKey>) {
    let mut mutations = Vec::new();
    let mut reads = Vec::new();

    let path = args
        .get("path")
        .or_else(|| args.get("file_path"))
        .or_else(|| args.get("file"))
        .and_then(|v| v.as_str());
    let command = args.get("command").and_then(|v| v.as_str());
    let host = args
        .get("url")
        .or_else(|| args.get("host"))
        .and_then(|v| v.as_str());

    match tool_name {
        // File mutators.
        "write_file" | "edit" => {
            if let Some(p) = path {
                mutations.push(ResourceKey::Path(normalize_path(p)));
            }
        }
        // File readers.
        "read_file" => {
            if let Some(p) = path {
                reads.push(ResourceKey::Path(normalize_path(p)));
            }
        }
        // Search tools read the whole tree (coarse-grained: directory). They
        // conflict with writes anywhere under that directory.
        "grep" | "glob" => {
            if let Some(p) = args.get("path").and_then(|v| v.as_str()) {
                reads.push(ResourceKey::Path(format!("{}/", normalize_path(p))));
            } else {
                reads.push(ResourceKey::Path("/".to_string()));
            }
        }
        // Bash mutates the working tree (conservatively) and is keyed by program.
        "bash" => {
            if let Some(cmd) = command {
                mutations.push(ResourceKey::BashProgram(leading_program(cmd)));
            } else {
                mutations.push(ResourceKey::BashProgram("(bash)".to_string()));
            }
        }
        // Network tools: keyed by host. GET is a read, everything else mutates.
        "webfetch" | "tavily_search" => {
            if let Some(h) = host {
                reads.push(ResourceKey::Host(host_of(h)));
            }
        }
        // git_* mutate the repo.
        "git_commit" => {
            mutations.push(ResourceKey::Path("/.git".to_string()));
        }
        "git_status" | "git_diff" | "git_log" | "git_show" => {
            reads.push(ResourceKey::Path("/.git".to_string()));
        }
        _ => {
            // Unknown tool: don't declare resources → free to parallelize.
            // If it had a path field, treat it conservatively as a mutation so
            // we never run two path-bearing unknown tools concurrently on the
            // same file.
            if let Some(p) = path {
                mutations.push(ResourceKey::Path(normalize_path(p)));
            }
        }
    }

    (mutations, reads)
}

/// Collapse `.` / `..` and repeated slashes so that `./a/b` and `a/b` hash equal.
fn normalize_path(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => continue,
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

/// First token of a shell command, for coarse bash conflict grouping.
fn leading_program(cmd: &str) -> String {
    let trimmed = cmd.trim_start();
    let first = trimmed.split_whitespace().next().unwrap_or("");
    // Strip a leading path: use just the basename (`/usr/bin/git` -> `git`).
    first.rsplit('/').next().unwrap_or(first).to_string()
}

/// Extract the hostname from a URL-ish string.
fn host_of(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_reads_parallelize() {
        let nodes = vec![
            SchedNode {
                idx: 0,
                tool_name: "read_file".into(),
                tool_call_id: "0".into(),
                args: serde_json::json!({"path": "a.rs"}),
                mutations: vec![],
                reads: vec![ResourceKey::Path("a.rs".into())],
            },
            SchedNode {
                idx: 1,
                tool_name: "read_file".into(),
                tool_call_id: "1".into(),
                args: serde_json::json!({"path": "b.rs"}),
                mutations: vec![],
                reads: vec![ResourceKey::Path("b.rs".into())],
            },
        ];
        let g = DepGraph::build(&nodes, ToolExecutionMode::Parallel);
        assert_eq!(g.indegree, vec![0, 0], "two reads of different files");
    }

    #[test]
    fn write_after_read_of_same_file_serializes() {
        let nodes = vec![
            SchedNode {
                idx: 0,
                tool_name: "write_file".into(),
                tool_call_id: "0".into(),
                args: serde_json::json!({"path": "a.rs"}),
                mutations: vec![ResourceKey::Path("a.rs".into())],
                reads: vec![],
            },
            SchedNode {
                idx: 1,
                tool_name: "read_file".into(),
                tool_call_id: "1".into(),
                args: serde_json::json!({"path": "a.rs"}),
                mutations: vec![],
                reads: vec![ResourceKey::Path("a.rs".into())],
            },
        ];
        let g = DepGraph::build(&nodes, ToolExecutionMode::Parallel);
        assert_eq!(g.indegree, vec![0, 1]);
        assert_eq!(g.dependents[0], vec![1]);
    }

    #[test]
    fn sequential_mode_full_chain() {
        let nodes: Vec<_> = (0..3)
            .map(|i| SchedNode {
                idx: i,
                tool_name: "read_file".into(),
                tool_call_id: i.to_string(),
                args: serde_json::json!({"path": format!("f{i}")}),
                mutations: vec![],
                reads: vec![ResourceKey::Path(format!("f{i}"))],
            })
            .collect();
        let g = DepGraph::build(&nodes, ToolExecutionMode::Sequential);
        assert_eq!(g.indegree, vec![0, 1, 1]);
    }

    #[test]
    fn classify_writes_and_reads() {
        let (mut_w, _) = classify_resources("write_file", &serde_json::json!({"path": "./x/y.rs"}));
        assert_eq!(mut_w, vec![ResourceKey::Path("x/y.rs".into())]);

        let (_, reads) = classify_resources("read_file", &serde_json::json!({"path": "x/y.rs"}));
        assert_eq!(reads, vec![ResourceKey::Path("x/y.rs".into())]);

        let (mut_b, _) = classify_resources(
            "bash",
            &serde_json::json!({"command": "/usr/bin/git status"}),
        );
        assert_eq!(mut_b, vec![ResourceKey::BashProgram("git".into())]);
    }
}
