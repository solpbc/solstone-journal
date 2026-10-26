// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The node test guard (`tests/node/solstone-node-test-guard.js`, preloaded
//! through `.cargo/config.toml`) keeps a failing or runaway DOM harness from
//! exhausting the machine. Each script here is bounded on its own, so a missing
//! guard fails these tests instead of taking the host down.

use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn node(script: &str, envs: &[(&str, &str)]) -> (Output, Duration) {
    let started = Instant::now();
    let mut command = Command::new("node");
    command.arg("-e").arg(script);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output().expect("node starts");
    (output, started.elapsed())
}

#[test]
fn cargo_preloads_the_guard_into_node() {
    let (output, _) = node(
        "process.stdout.write(String(globalThis.__solstoneNodeTestGuard === true))",
        &[],
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "true",
        "NODE_OPTIONS from .cargo/config.toml must preload tests/node/solstone-node-test-guard.js; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// A small fake document: every node links its parent, children, siblings and
// document. node's own strictEqual message inspects both sides 1000 levels deep
// and only tracks cycles along one path, so on a graph like this a single
// failing comparison grows without bound. The guard reports it in a few lines.
const FAKE_DOCUMENT: &str = r#"
const assert = require('node:assert');
const doc = { nodes: [] };
const make = (parent) => {
  const node = { parent, children: [], ownerDocument: doc, listeners: { click: [() => {}] }, attributes: { class: 'card' } };
  doc.nodes.push(node);
  if (parent) {
    const previous = parent.children[parent.children.length - 1];
    if (previous) { previous.nextSibling = node; node.previousSibling = previous; }
    parent.children.push(node);
  }
  return node;
};
const root = make(null);
doc.root = root;
let frontier = [root];
for (let depth = 0; depth < 5; depth += 1) {
  const next = [];
  for (const parent of frontier) for (let i = 0; i < 4; i += 1) next.push(make(parent));
  frontier = next;
}
assert.strictEqual(frontier[0], frontier[1]);
"#;

#[test]
fn a_failing_node_comparison_fails_fast_with_a_short_message() {
    let (output, elapsed) = node(FAKE_DOCUMENT, &[("SOLSTONE_NODE_TEST_SECONDS", "60")]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "the comparison must fail");
    assert!(stderr.contains("strictEqual failed"), "stderr: {stderr}");
    assert!(
        !stderr.contains("node test guard: killed"),
        "the assertion itself must be bounded: {stderr}"
    );
    assert!(stderr.len() < 20_000, "message is {} bytes", stderr.len());
    assert!(elapsed < Duration::from_secs(30), "took {elapsed:?}");
}

#[test]
fn a_runaway_allocation_is_killed_at_the_memory_cap() {
    // Allocates at most 1 GiB, then blocks for ten seconds: the guard should
    // kill it near 256 MB, long before either bound ends the script itself.
    let script = r#"
const keep = [];
for (let i = 0; i < 128; i += 1) keep.push(Buffer.alloc(8 * 1048576, 1));
Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10000);
"#;
    let (output, elapsed) = node(script, &[("SOLSTONE_NODE_TEST_RSS_MB", "256")]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "the guard must kill it; stderr: {stderr}"
    );
    assert!(
        stderr.contains("node test guard: killed"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("MB cap"), "stderr: {stderr}");
    assert!(elapsed < Duration::from_secs(30), "took {elapsed:?}");
}

#[test]
fn a_blocked_loop_is_killed_at_the_time_limit() {
    // Spins for at most twenty seconds; the guard should kill it after two.
    let script = "const end = Date.now() + 20000; while (Date.now() < end) {}";
    let (output, elapsed) = node(script, &[("SOLSTONE_NODE_TEST_SECONDS", "2")]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "the guard must kill it; stderr: {stderr}"
    );
    assert!(
        stderr.contains("node test guard: killed"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("s limit"), "stderr: {stderr}");
    assert!(elapsed < Duration::from_secs(15), "took {elapsed:?}");
}
