# Cells Prototype — Domain Glossary

A prototype of a DoenetML core organized around data (cells) rather than
user-facing components. Terms below are the canonical vocabulary. This file is
a glossary only; design decisions live in `docs/adr/`.

## Terms

**Cell** — A single double-precision value in the document's state. Cells are
the nodes of the dependency graph. Every cell is either essential or derived.

**Essential cell** — A cell with no dependencies. Its value is part of the
minimum independent data needed to recreate the document state. Initialized
from an attribute literal or a component default. The only cells a write can
ultimately land on.

**Derived cell** — A cell whose value is computed by an operator from other
cells. Never written directly; a request naming a derived cell is inverted
until it reaches essential cells.

**Operator** — The numeric function that computes one derived cell from its
input cells. Every operator may carry an inverse. Identity references are not
operators: they alias. In the prototype, operators appear in a document as the
prototype-only `<op>` tag.

**Literal parameter** — A constant written in the document that an operator
uses (a scale factor, clamp bounds). Literal parameters are document text, not
state, so they are not cells.

**Alias** — Two state variables that are exactly equal by construction (a
reference such as `x="$p1.x"`, or a component copy `$p1`) share one cell.
Aliasing is structural, decided at load time, never by comparing values.

**Prop** — A named state variable of a component, mapping to one cell or an
ordered list of cells (a point's `coords` maps to its `x` and `y` cells). Props
are a naming layer over cells; they are not graph nodes.

**Component** — A DoenetML tag instance (`number`, `numberInput`, `graph`,
`point`). Components own props and renderer identity but no computation of
their own.

**Reference** — A `$name` or `$name.prop` in the document. A component-level
reference (`$p1`) produces a copy sharing every cell with the referent; a
prop-level reference (`$p1.x`) aliases that one prop. A bare `$name` resolves
to the referent's default prop (`value` for numbers and inputs).

**Copy** — The component produced by a component-level reference. A copy may
override individual props with attributes (`<point extend="$p1" y="3"/>`);
overridden props get their own cells, the rest stay aliased.

**Tick** — One pass of the core in response to an action: resolve requests to
essential cells, recompute derived cells, report changed cells.

**Request** — A renderer's ask to change a cell to a value. Requests are
cell-addressed. The core resolves a request by inverting through operators
until essential cells are reached, then recomputes.

**Inverse** — The rule an operator uses to turn a requested output value into
requested input values. Binary operators pick one input to receive the write.
An operator with no inverse makes its output non-draggable.

**Schedule** — The topological order in which derived cells are recomputed
after essential cells change. Fixed once the document is loaded.

**Render manifest** — The one-time description handed to the renderer at load:
the component tree with each rendered prop's cell indices.

**Cycle** — A dependency loop among cells. Rejected at load time.
