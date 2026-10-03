//! Shared fixture loading for benches and the stats binary.

use std::path::PathBuf;

pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// The binary wire-format bytes of a fixture, if `scripts/gen-fixtures.sh`
/// produced them.
pub fn fixture_binary(spec: &str) -> Option<Vec<u8>> {
    std::fs::read(fixtures_dir().join(format!("{spec}.cdast"))).ok()
}

/// (spec, DAST JSON) for every fixture, sorted by spec name. Set
/// `CELLS_FIXTURES` to a comma-separated list of specs to restrict the set.
pub fn fixtures() -> Vec<(String, String)> {
    let filter: Option<Vec<String>> = std::env::var("CELLS_FIXTURES")
        .ok()
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect());
    let mut out = Vec::new();
    let dir = fixtures_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("no fixtures in {}; run scripts/gen-fixtures.sh", dir.display());
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "json") {
            let spec = p.file_stem().unwrap().to_string_lossy().to_string();
            if filter.as_ref().is_some_and(|f| !f.contains(&spec)) {
                continue;
            }
            out.push((spec, std::fs::read_to_string(&p).unwrap()));
        }
    }
    out.sort_by_key(|a| spec_key(&a.0));
    out
}

/// Sort "chain-100" before "chain-1000" by (shape, numeric size).
pub fn spec_key(spec: &str) -> (String, u64, u64) {
    let (shape, size) = spec.split_once('-').unwrap_or((spec, "0"));
    let (a, b) = size.split_once('x').unwrap_or((size, "0"));
    (shape.to_string(), a.parse().unwrap_or(0), b.parse().unwrap_or(0))
}

/// The cell a drag request would target in each fixture shape: the end of
/// the chain, the fan-out root, or the first point's x.
pub fn drag_target(doc: &cells_core::Document, spec: &str) -> cells_core::CellIdx {
    let shape = spec.split('-').next().unwrap();
    match shape {
        "chain" => doc.cell("p", "x").unwrap(),
        "fanout" => doc.cell("n", "value").unwrap(),
        "aliases" => doc.cell("p", "x").unwrap(),
        "grid" => doc.cell("p0", "x").unwrap(),
        _ => doc.cell("p0", "x").unwrap(),
    }
}
