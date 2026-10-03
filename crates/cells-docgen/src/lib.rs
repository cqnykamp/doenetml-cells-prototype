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
/// negate so that every value changes when the input does; the last
/// operator is a clamp so the point stays inside the graph for dragging.
pub fn chain(l: usize) -> String {
    let mut s = String::from("<numberInput name=\"n\" value=\"1\"/>\n");
    let mut prev = "n".to_string();
    for i in 0..l {
        let name = format!("c{i}");
        let op = if i + 1 == l {
            "kind=\"clamp\" lo=\"-9\" hi=\"9\""
        } else {
            match i % 3 {
                0 => "kind=\"offset\" k=\"1\"",
                1 => "kind=\"scale\" k=\"1.0001\"",
                _ => "kind=\"negate\"",
            }
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
/// point, all points in one graph. Cells: N(L + 2) + 4. Derived: NL.
pub fn grid(n: usize, l: usize) -> String {
    let mut s = String::new();
    let mut points = String::from("<graph name=\"g\">\n");
    for j in 0..n {
        let _ = writeln!(s, "<numberInput name=\"n{j}\" value=\"{}\"/>", j % 10);
        let mut prev = format!("n{j}");
        for i in 0..l {
            let name = format!("c{j}_{i}");
            let op = if i % 2 == 0 { "kind=\"offset\" k=\"1\"" } else { "kind=\"negate\"" };
            let _ = writeln!(s, "<op name=\"{name}\" {op} args=\"${prev}\"/>");
            prev = name;
        }
        let _ = writeln!(points, "  <point name=\"p{j}\" x=\"${prev}\" y=\"{}\"/>", j % 20);
    }
    points.push_str("</graph>\n");
    s + &points
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

/// Translate a generated document into DoenetML the current core accepts:
/// each `<op>` becomes a `<number>` with the equivalent math expression.
/// Clamp has no one-line equivalent and becomes a plain copy, so chains lose
/// their final clamp; the values differ but the dependency shape is the same.
pub fn to_legacy(doc: &str) -> String {
    let mut out = String::new();
    // The current core rejects non-graphical children of <graph>, so numbers
    // that the generator placed inside a graph are hoisted above it.
    let mut graph_lines: Vec<String> = Vec::new();
    let mut hoisted: Vec<String> = Vec::new();
    let mut in_graph = false;
    for line in doc.lines() {
        // Split lines that hold an <op .../> followed by another element.
        let mut pieces: Vec<&str> = Vec::new();
        let mut rest = line;
        while let Some(i) = rest.find("/><") {
            pieces.push(&rest[..i + 2]);
            rest = &rest[i + 2..];
        }
        pieces.push(rest);
        for piece in pieces {
            let t = piece.trim();
            if t.is_empty() {
                continue;
            }
            let translated = if let Some(rest) = t.strip_prefix("<op ") {
                let attr = |k: &str| -> Option<String> {
                    let pat = format!("{k}=\"");
                    let i = rest.find(&pat)? + pat.len();
                    let j = rest[i..].find('"')? + i;
                    Some(rest[i..j].to_string())
                };
                let name = attr("name").unwrap_or_default();
                let kind = attr("kind").unwrap_or_default();
                let args: Vec<String> = attr("args").unwrap_or_default().split_whitespace().map(String::from).collect();
                let a = args.first().cloned().unwrap_or_default();
                let b = args.get(1).cloned().unwrap_or_default();
                let expr = match kind.as_str() {
                    "add" => format!("{a} + {b}"),
                    "sub" => format!("{a} - {b}"),
                    "mul" => format!("{a} * {b}"),
                    "negate" => format!("-{a}"),
                    "scale" => format!("{} * {a}", attr("k").unwrap_or_default()),
                    "offset" => format!("{a} + {}", attr("k").unwrap_or_default()),
                    "lerp" => {
                        let t = attr("t").unwrap_or_default();
                        format!("{a} + {t} * ({b} - {a})")
                    }
                    _ => a.clone(), // clamp and unknown: plain copy
                };
                Some(format!("<number name=\"{name}\">{expr}</number>"))
            } else {
                None
            };
            let is_number = translated.is_some();
            // The current core has no numberInput; mathInput with prefill is
            // the equivalent input.
            let text = translated.unwrap_or_else(|| t.replace("<numberInput ", "<mathInput ").replace(" value=\"", " prefill=\""));
            let starts_graph = t.starts_with("<graph");
            let ends_graph = t.contains("</graph>");
            if starts_graph {
                in_graph = true;
            }
            if in_graph && is_number {
                hoisted.push(text);
            } else if in_graph {
                graph_lines.push(text);
            } else {
                out.push_str(&text);
                out.push('\n');
            }
            if ends_graph {
                in_graph = false;
                for h in hoisted.drain(..) {
                    out.push_str(&h);
                    out.push('\n');
                }
                for g in graph_lines.drain(..) {
                    out.push_str(&g);
                    out.push('\n');
                }
            }
        }
    }
    out
}
