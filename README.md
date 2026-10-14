# Cells prototype

**Moved.** On 2026-10-14 the code moved to the DoenetML repository as
`packages/cells-core` and `packages/cells-viewer` (branch
`cells-prototype-adapter` of the fork at github.com/cqnykamp/DoenetML),
next to the test adapter that measures its parity with the current core.
Development continues there; this repository is the record of the
prototype rounds that produced it and is not updated.

A DoenetML core organized around a flat list of `f64` cells instead of
components: components and props name cells, and the dependency graph is a
list of operators with inverses. The code is laid out as the two packages
it became in the DoenetML repository.

- `packages/cells-core`: the core (Rust, compiled to wasm), its
  `ARCHITECTURE.md` (start there), `CONTEXT.md` (the vocabulary) and
  `docs/adr/` (design decisions).
- `packages/cells-viewer`: the React + JSXGraph viewer and the Chromium
  measurements.
- `docs/history/`: the brief and plan of each round of the prototype.
- `RESULTS.md`, `results/NOTES.md`: the measurements and verdicts of those
  rounds. `RESULTS.md` is frozen at the commit in its header;
  `scripts/render-results.py` is the generator it was made with.

## Running

Everything needs `node` on PATH and a built DoenetML checkout for the
parser: `DOENETML_DIR` names it (default: `../../ml`, a sibling of this
repository). The Rust side also needs `wasm-pack` and the
`wasm32-unknown-unknown` target.

```sh
npm install                                 # both packages
npm run lint && npm test                    # fmt, clippy, cargo test

cd packages/cells-core
npm run fixtures                            # ~1 minute; generates fixtures/*.{doenet,json,cdast}
npm run bench                               # criterion: startup, tick, rebuild over the fixture set
npm run stats > ../../results/raw/stats.jsonl
npm run golden-diff -- [rev]                # behavior dump of every golden document vs a base commit
npm run perf-diff -- [rev]                  # quick perf check vs a base commit (~2 min)
cargo run --release -p cells-bench --example sir_perf   # the SIR document, native
npm run build:rust                          # the wasm, into pkg/

cd packages/cells-viewer
npm run dev                                 # the viewer at :5173
npm run e2e                                 # headless drag measurement
npm run build && npm run preview & npm run sir-perf     # the SIR document, in Chromium
```

Restrict a run with `CELLS_FIXTURES=chain-1000,points-100 npm run bench` or
`CELLS_E2E_FIXTURES=... CELLS_E2E_EVALS=full CELLS_E2E_FORMATS=cdast,json npm run e2e`.
The viewer takes `?doc=<spec>`, `?fmt=json|cdast` and `?eval=full|dirty-closure`.
`CELLS_BUILD_PROFILE=1` prints per-phase build timings from any binary.

## License

Copyright (C) 2026 Charles Nykamp. Licensed under the GNU Affero General Public
License, version 3 or (at your option) any later version. See [LICENSE](LICENSE).
