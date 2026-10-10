# Cells prototype

A prototype DoenetML core organized around a flat list of `f64` cells instead
of components. See `docs/history/instructions.md` for the goal, `CONTEXT.md` for the
vocabulary, and `docs/adr/` for recorded decisions.

## Layout

- `crates/cells-core` — the core. `dast/` reads the DAST (JSON or the
  binary `CDST` wire format, ADR 0002); `components/` describes each kind
  (one `KINDS` row: tags, props, flags) and names prop positions
  (`components::prop`); `build/` compiles templates once
  (`build/compile/`, into the types in `plan.rs`), stamps them per scope
  (`build/expand/`) and emits cells and the program (`build/emit.rs`), rebuilding the whole document on
  structural change (ADR 0004); `program/` is the instruction set the build
  emits and a tick runs: `ops.rs` and `geo.rs` hold the operators and their
  inverses (ADR 0003, 0006), `program/mod.rs` the scheduled program; `tick/`
  holds the run-time machinery, `eval.rs` the evaluators, `invert.rs` the
  request engine and `snap.rs` the sticky rule; `document/` loads the
  document (`load.rs`), runs ticks (`request.rs`, with the sticky pre-pass
  in `sticky.rs`) and answers questions about it (`table.rs`,
  `sections.rs`, `paths.rs`); `testing/` holds the reference oracle and test
  helpers. Math cells are handles into a symbolic engine
  (`cells-sym`, behind the `SymEngine` trait; ADR 0008).
- `crates/cells-sym` — the symbolic engine (engine A) and curve tapes;
  `crates/cells-sym-mer` wraps math-expressions-rs (engine R) as its oracle.
- `crates/cells-wasm` — wasm-bindgen binding: zero-copy views of the cells and
  the component table, cell-addressed requests (see `docs/adr/0001-*`).
- `crates/cells-docgen` — synthetic DoenetML generator (points, chain, fanout,
  aliases, grid; plan 2 adds sliderchain, sliderstack, repeat, recur,
  intchain, mathchain, hidden) used for all benchmarks, plus the
  current-core counterparts for the baseline (`--legacy`).
- `crates/cells-bench` — criterion benches (`startup`, `tick`, `rebuild`), a
  `stats` binary for structure and memory, and examples: `regress` (quick
  perf), `golden` (behavior dump) and a few measurement tools.
- `web/` — Vite + React renderers (SVG graph with draggable points, number,
  numberInput, slider, booleanInput, math, repeat and collect) and the
  Playwright end-to-end measurement, including structural ticks (rebuilds).

Tags the core understands: `document`, `graph`, `point` (x, y, hide),
`number`, `numberInput`, `booleanInput`, `slider` (numeric mode), `math`,
`evaluate`, `repeatForSequence`, `collect`, and the prototype-only `op`;
plan 3 to 5 added `line`, `lineSegment`, `circle`, `polygon`, `pointList`,
`p`, `setup`, `stickyGroup`, `mathInput`, `function`, `derivative` and
`answer`; plan 6 added `conditionalContent` (`case`, `else`), `select`
(`option`), `group` and literal `text` (see ADR 0009); sections
(`section`, `subsection`, `subsubsection`, `problem`, `exercise`,
`example`) with credit and numbering through choices (`build/expand/scoring.rs`).
See `docs/history/plan-2.md` for the second round's scope and decisions.
- `scripts/parse-dast.mjs` — runs the existing TypeScript DoenetML parser from
  a sibling DoenetML checkout (`DOENETML_DIR`, default `../../ml`) and prints
  normalized DAST JSON, or the binary wire format with `--binary`.
- `scripts/cdast-encode.mjs` — the DAST to binary encoder, shared by the
  script and the web app.
- `scripts/gen-fixtures.sh` — generates `fixtures/*.doenet` and `*.json`.
- `scripts/golden-diff.sh` — runs `cells-bench/examples/golden.rs` (every
  component's props by tree path, after load and after a fixed script of
  requests, on `crates/cells-bench/golden/*.doenet` and the smallest fixture of
  each shape) on the working tree and on a base commit, and diffs them.
- `scripts/perf-diff.sh` — the `regress` example on a base commit and the
  working tree, interleaved, as new/base ratios.
- `scripts/render-results.py` — collects criterion, stats and e2e output into
  `results/raw/` and regenerates `RESULTS.md` (commentary in `results/NOTES.md`).

## Running

```sh
cargo test                      # needs `node` on PATH and a built DoenetML parser
echo '<point name="p" x="1"/>' | node scripts/parse-dast.mjs

scripts/gen-fixtures.sh         # ~1 minute; writes fixtures/*.{doenet,json,cdast}
cargo bench -p cells-bench      # ~2.5 hours for the full sweep over 93 fixtures (startup, tick, rebuild)
cargo run --release -p cells-bench --bin stats > results/raw/stats.jsonl
CELLS_BUILD_PROFILE=1 cargo run --release -p cells-bench --example rebuild_loop -- repeat-10000 3   # per-phase build timings

cd web && pnpm install && pnpm wasm && pnpm dev     # interactive renderer at :5173
cd web && pnpm e2e                                  # headless drag measurement
scripts/render-results.py                           # regenerate RESULTS.md
scripts/golden-diff.sh [rev]    # behavior dump of every golden document vs a base commit (default HEAD)
scripts/perf-diff.sh [rev]      # quick perf check: regress on 10 fixtures, base commit vs working tree (~2 min)
```

Restrict a run with `CELLS_FIXTURES=chain-1000,points-100 cargo bench ...` or
`CELLS_E2E_FIXTURES=... CELLS_E2E_EVALS=full CELLS_E2E_BACKENDS=main CELLS_E2E_FORMATS=cdast,json pnpm e2e`.
The current-core baseline is `cd web && node baseline/measure.mjs`; the slider
differential test against the current core is `cd web && node baseline/slider-diff.mjs`
(after `cargo build --release -p cells-bench --example scenario_run`).
