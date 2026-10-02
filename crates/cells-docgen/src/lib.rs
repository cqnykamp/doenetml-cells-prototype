//! Synthetic DoenetML documents along three scale axes: number of
//! independent points (N), chain length (L), and fan-out (K). Each generator
//! returns DoenetML text so the same documents can be fed to the current core.

use std::fmt::Write;

/// N free points in one graph. Cells: 2N + 4. No derived cells.
pub fn points(n: usize) -> String {
    let mut s = String::from("<graph name=\"g\">\n");
    for i in 0..n {
        let _ = writeln!(s, "  <point name=\"p{i}\" x=\"{}\" y=\"{}\"/>", i % 20, (i * 7) % 20);
    }
    s.push_str("</graph>\n");
    s
}

/// One input feeding a chain of L operators, ending at a point's x.
/// Cells: L + 1 + 4. Derived: L. The chain alternates offset, scale and
/// negate so that every value changes when the input does.
pub fn chain(l: usize) -> String {
    let mut s = String::from("<numberInput name=\"n\" value=\"1\"/>\n");
    let mut prev = "n".to_string();
    for i in 0..l {
        let name = format!("c{i}");
        let op = match i % 3 {
            0 => "kind=\"offset\" k=\"1\"",
            1 => "kind=\"scale\" k=\"1.0001\"",
            _ => "kind=\"negate\"",
        };
        let _ = writeln!(s, "<op name=\"{name}\" {op} args=\"${prev}\"/>");
        prev = name;
    }
    let _ = writeln!(s, "<graph name=\"g\"><point name=\"p\" x=\"${prev}\" y=\"$n\"/></graph>");
    s
}

/// One input feeding K operators, each driving one point's x.
/// Cells: 1 + K + K + 4. Derived: K.
pub fn fanout(k: usize) -> String {
    let mut s = String::from("<numberInput name=\"n\" value=\"1\"/>\n<graph name=\"g\">\n");
    for i in 0..k {
        let _ = writeln!(
            s,
            "  <op name=\"o{i}\" kind=\"scale\" k=\"{}\" args=\"$n\"/><point name=\"p{i}\" x=\"$o{i}\" y=\"{}\"/>",
            1.0 + (i % 10) as f64 / 10.0,
            i % 20
        );
    }
    s.push_str("</graph>\n");
    s
}

/// One point and N copies of it. Cells: 2 + 4 regardless of N; exercises
/// alias merging and the size of the render tree relative to the cell array.
pub fn aliases(n: usize) -> String {
    let mut s = String::from("<graph name=\"g\">\n  <point name=\"p\" x=\"1\" y=\"2\"/>\n");
    for _ in 0..n {
        s.push_str("  $p\n");
    }
    s.push_str("</graph>\n");
    s
}

/// N independent chains of length L, each from its own input to its own
/// point. Cells: N(L + 2) + 4. Derived: NL.
pub fn grid(n: usize, l: usize) -> String {
    let mut s = String::new();
    for j in 0..n {
        let _ = writeln!(s, "<numberInput name=\"n{j}\" value=\"{}\"/>", j % 10);
        let mut prev = format!("n{j}");
        for i in 0..l {
            let name = format!("c{j}_{i}");
            let op = if i % 2 == 0 { "kind=\"offset\" k=\"1\"" } else { "kind=\"negate\"" };
            let _ = writeln!(s, "<op name=\"{name}\" {op} args=\"${prev}\"/>");
            prev = name;
        }
        let _ = writeln!(s, "<point name=\"p{j}\" x=\"${prev}\" y=\"{}\"/>", j % 20);
    }
    s
}

/// Parse a CLI-style spec such as `chain-1000` or `grid-100x10`.
pub fn from_spec(spec: &str) -> Option<String> {
    let (shape, size) = spec.split_once('-')?;
    Some(match shape {
        "points" => points(size.parse().ok()?),
        "chain" => chain(size.parse().ok()?),
        "fanout" => fanout(size.parse().ok()?),
        "aliases" => aliases(size.parse().ok()?),
        "grid" => {
            let (n, l) = size.split_once('x')?;
            grid(n.parse().ok()?, l.parse().ok()?)
        }
        _ => return None,
    })
}

/// The default sweep used by `scripts/gen-fixtures.sh` and the benches.
pub const DEFAULT_SWEEP: &[&str] = &[
    "points-10", "points-100", "points-1000", "points-10000", "points-50000",
    "chain-10", "chain-100", "chain-1000", "chain-10000", "chain-100000",
    "fanout-10", "fanout-100", "fanout-1000", "fanout-10000", "fanout-50000",
    "aliases-10", "aliases-1000", "aliases-10000",
    "grid-100x10", "grid-1000x10", "grid-100x100", "grid-1000x100",
];
