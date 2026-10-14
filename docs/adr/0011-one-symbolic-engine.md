# One symbolic engine: math-expressions-rs

Status: accepted; implementation is the step after the cleanup of
October 2026. Supersedes the engine verdict of plan 5 that ADR 0008 relies
on.

The core will hold one symbolic engine, the Rust port of math-expressions
(`math-expressions-rs`, Doenet's own library, pinned by git revision), and
call it directly. The `SymEngine` trait, the hash-consed arena (engine A),
and the core's own math-text parser go. The compiled curve tape stays, since
it compiles from the core's `Tree` and is independent of the engine.

We chose this because engine A's advantage was structural: equal expressions
got equal handles, so a tick could tell for free whether a symbolic
instruction's output changed. In practice DoenetML compares mathematically
by numeric sampling, not structurally, so that identity is not used for
anything the document can observe. Against that, one engine across all of
DoenetML gives one parser, one simplifier and one equality, which is what
parity with the current core needs. Plan 5 measured engine A as faster per
operation and an order of magnitude leaner in memory; that cost is accepted
and not re-measured now, because math-expressions-rs is itself expected to
change. The bench set keeps a symbolic family so growth stays visible.

## Considered options

- **Keep both engines behind the trait**, A by default. Rejected: the trait
  exists only to compare them, and a test oracle that needs a sibling
  checkout is a test nobody runs.
- **Keep the core's own parser** and convert its trees into the engine.
  Rejected: the parser is the piece most likely to diverge from DoenetML's
  handling of math text.

## Consequences

- `cells-sym` shrinks to `Tree`, the tape and nothing else, or folds into
  the core.
- The oracle test (A against R) disappears with A; the tests that ran the
  core with R become the ordinary symbolic tests.
- The crate enables `serde_json/arbitrary_precision`, which cargo's feature
  unification spreads to the whole wasm binary.
