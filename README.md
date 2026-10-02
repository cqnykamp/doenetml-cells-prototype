# Cells prototype

A prototype DoenetML core organized around a flat list of `f64` cells instead
of components. See `instructions.md` for the goal, `CONTEXT.md` for the
vocabulary, and `docs/adr/` for recorded decisions.

## Layout

- `crates/cells-core` — the core: DAST import, reference resolution, alias
  merging, operator program, scheduler.
- `crates/cells-wasm` — wasm-bindgen binding: zero-copy cell view, render
  manifest, cell-addressed requests (see `docs/adr/0001-*`).
- `crates/cells-docgen` — synthetic DoenetML generator (points, chain, fanout,
  aliases, grid) used for all benchmarks.
- `crates/cells-bench` — criterion benches (`startup`, `tick`) and a `stats`
  binary for structure and memory.
- `web/` — Vite + React renderers (SVG graph with draggable points, number,
  numberInput) and the Playwright end-to-end measurement.
- `scripts/parse-dast.mjs` — runs the existing TypeScript DoenetML parser from
  a sibling DoenetML checkout (`DOENETML_DIR`, default `../../ml`) and prints
  normalized DAST JSON.
- `scripts/gen-fixtures.sh` — generates `fixtures/*.doenet` and `*.json`.
- `scripts/render-results.py` — collects criterion, stats and e2e output into
  `results/raw/` and regenerates `RESULTS.md` (commentary in `results/NOTES.md`).

## Running

```sh
cargo test                      # needs `node` on PATH and a built DoenetML parser
echo '<point name="p" x="1"/>' | node scripts/parse-dast.mjs

scripts/gen-fixtures.sh         # ~1 minute; writes fixtures/
cargo bench -p cells-bench      # ~30 minutes for the full sweep
cargo run --release -p cells-bench --bin stats > results/raw/stats.jsonl

cd web && pnpm install && pnpm wasm && pnpm dev     # interactive renderer at :5173
cd web && pnpm e2e                                  # headless drag measurement
scripts/render-results.py                           # regenerate RESULTS.md
```

Restrict a run with `CELLS_FIXTURES=chain-1000,points-100 cargo bench ...` or
`CELLS_E2E_FIXTURES=... CELLS_E2E_EVALS=full pnpm e2e`.
