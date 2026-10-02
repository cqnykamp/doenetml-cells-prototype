# Cells prototype

A prototype DoenetML core organized around a flat list of `f64` cells instead
of components. See `instructions.md` for the goal, `CONTEXT.md` for the
vocabulary, and `docs/adr/` for recorded decisions.

## Layout

- `crates/cells-core` — the core: DAST import, reference resolution, alias
  merging, operator program, scheduler.
- `scripts/parse-dast.mjs` — runs the existing TypeScript DoenetML parser from
  a sibling DoenetML checkout (`DOENETML_DIR`, default `../../ml`) and prints
  normalized DAST JSON.

## Running

```sh
cargo test              # needs `node` on PATH and a built DoenetML parser
echo '<point name="p" x="1"/>' | node scripts/parse-dast.mjs
```
