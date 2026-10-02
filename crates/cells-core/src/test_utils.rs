//! Helpers for tests and benches: turn DoenetML source into DAST JSON by
//! running the TypeScript parser through node, mirroring how the current core
//! gets its DAST.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::io::Write;

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// Parse DoenetML source to DAST JSON via `scripts/parse-dast.mjs`.
pub fn dast_json(source: &str) -> String {
    let script = repo_root().join("scripts/parse-dast.mjs");
    let mut child = Command::new("node")
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("node must be on PATH to parse DoenetML");
    child.stdin.take().unwrap().write_all(source.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "parse-dast.mjs failed");
    String::from_utf8(out.stdout).unwrap()
}

/// Parse and load a document from DoenetML source.
pub fn load(source: &str) -> crate::Result<crate::Document> {
    crate::Document::from_dast_json(&dast_json(source))
}
