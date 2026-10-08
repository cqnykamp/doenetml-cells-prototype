# Cells Prototype — Domain Glossary

A prototype of a DoenetML core organized around data (cells) rather than
user-facing components. Terms below are the canonical vocabulary. This file is
a glossary only; design decisions live in `docs/adr/`.

## Terms

**Cell** — A single double-precision value in the document's state. Cells are
the nodes of the dependency graph. Every cell is essential, fixed or derived.
A cell's number may stand for an integer, a boolean (0 or 1) or a handle into
the expression arena; the meaning is a property of the operators around it,
not of the storage.

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
requested input values. Most operators write one input, their first argument;
a fan-out inverse writes several. Every derived cell has an essential
ancestor, because literal parameters are not cells, so inversion fails only
dynamically (division by zero); such a request is dropped.

**Fan-out inverse** — An inverse that turns one requested output value into
requests on several inputs at once (a circle's radius moving both coordinates
of its through point). Each resulting request is then inverted on its own.

**Local inverse** — An inverse that uses only the operator's own inputs, their
current values and the requested value. Plan 3's finding: every inverse is
local; the one other thing a *request* needs is the realized value of the
points requested with it (see point group, lookahead), never state saved
from an earlier tick.

**Point group** — Points requested together in one tick: a whole-shape drag
from the renderer, or the points one inverse moves at once. The request
engine keeps a group together when a strict subset of it is held back by a
constraint, by the same shift. A soft preference about drag behavior, not a
document invariant; the same requests sent singly move only what they name.
See ADR 0006.

**Sticky group** — A container whose members (points, polygons, polylines,
line segments) snap, when one of them is dragged, to the vertices and edges of
the *other* members within a threshold. A dragged member never attracts
itself. Snapping acts only on requests: it never moves a member that nobody
asked to move, so it is not a document invariant.

**Rigid inverse** — The inverse of the one identity instruction a `rigid`
polygon owns: it projects the requested change onto a rigid motion or
similarity. The document asked for the coupling, so it lives in the graph.

**Multi-output instruction** — An instruction that produces several cells.
Requests in one tick on its outputs are gathered into one vector, with
unspecified outputs at their current values, and inverted once.

**Lookahead** — Asking, during inversion, what value a cell would actually
take if a given value were requested of it: the request is inverted on a
scratch copy of the cells and the affected instructions are evaluated
forward. No cell is written and no tick runs. The request engine uses it on
a point group to learn which points are constrained before queuing the rest.

**Realized value** — The value a cell actually takes after a request, which
differs from the requested value when a projection inverse lies on the chain
(a snapped point). Lookahead computes it without a tick.

**Build-time variety** — Complication in a component's current inverse that
comes from the many ways the author may specify the component (a line by two
points, by a point and a slope, by an equation). In the cell model each way is
a different operator chain chosen when the document is built, so the variety
never reaches an inverse.

**Projection inverse** — The inverse rule for an idempotent operator (Round,
Clamp, Snap): the requested output value is passed through the operator itself
and the result is requested of the input. See ADR 0003.

**Iteration** — One expansion of a repeat's template, identified by its
1-based position. An iteration's essential keys include that position.

**Iteration count** — The structural cell holding how many iterations a repeat
has: the floor of its driving value, clamped to zero and the repeat's cap, with
a non-number counting as zero.

**Math cell** — A cell whose value is a handle into the expression arena rather
than a number. A math cell may be essential (a `mathInput`'s expression),
fixed (an expression written in the document) or derived (the result of a
symbolic instruction). Operators on math cells live in the same program and
schedule as numeric operators.

**Symbolic instruction** — An instruction whose output is a math cell computed
from other math cells (simplify, expand, substitute, derivative), or whose
output is a number computed from math cells (evaluate, equals). Symbolic
instructions run inside a tick, like any other instruction.

**Expression arena** — The core's store of symbolic expressions. Math cells
point into it; the core treats expressions as opaque and reaches them only
through the operations the symbolic engine offers.

**Lowering** — Replacing a `<math>` whose leaves are all number literals or
number-typed components with numeric operators at build time, so it never
becomes a math cell. Decided at build time, never from runtime values.

**Fixed cell** — A cell that holds a constant which is not state: an
iteration's index, a collect's count, the shared missing-referent NaN. Fixed
cells have no producer and no essential key; a request that reaches one is
dropped.

**Template** — The body of a repeat (or the document itself) as compiled
once from the DAST: its elements, their props' sources, and their references
as paths relative to the template nesting. A template is instantiated once
per scope. Templates are a build-time notion; the renderer never sees them.

**Scope** — A naming region. The document is scope 0; every iteration of a
repeat is a scope. A name inside a repeat template exists once per scope, so
`p` in iteration 3 is `$r[3].p`. A bare `$p` resolves from the referencing
component's scope outward. Scope ids are stable for the life of a document.

**Missing referent** — An indexed reference that names no component
(`$r[32]` when there are ten iterations). It resolves to the shared fixed
NaN cell; after a rebuild that creates iteration 32 it aliases the real cell.

**Dropped request** — A request whose inverse was undefined for the current
values. It changes nothing and is reported back with the tick.

**Schedule** — The topological order in which derived cells are recomputed
after essential cells change. Fixed once the document is loaded.

**Component table** — The columnar description of components (kind, name,
parent, children, prop cell indices) that the renderer reads directly, the
same way it reads cells. It replaces an earlier serialized render manifest.

**Wire format** — The encoding in which a parsed document reaches the core:
either the DAST JSON of the current DoenetML worker or the compact binary
form described in ADR 0002.

**Rebuild** — A reconstruction of the cell array, program and component table
from the document when its structure changes (the iteration count of a repeat
moves). A rebuild happens inside the tick that changed the count and must fit
the same interactive budget as any other tick. Values of essential cells
survive a rebuild through their essential keys. See ADR 0004.

**Essential key** — The identity of an essential cell independently of its
index: the scope it lives in and its slot within that scope's template (which
element, which prop). Essential values are kept by key across rebuilds, so an
iteration that disappears and reappears returns with its last values. Whether
keys also carry values across sessions is undecided.

**Structural depth** — Of a repeat: how many repeats must be expanded in
sequence before its count can be computed, plus one. Nesting adds one; a
count that reads a cell inside another repeat's iterations adds one. A
document loads in depth + 1 build passes. The second kind of link is reported
as a warning, since authors can avoid it.

**Structural cell** — A cell whose value determines document structure, such
as the iteration count of a repeat. A change to a structural cell triggers a
rebuild.

**Cycle** — A dependency loop among cells. Rejected at load time.

**Branch** — One alternative of a choice: a `<case>`, `<else>` or `<option>`
and the content inside it. A branch is a naming region, like an iteration.

**Load-time choice** — A choice made once, while the document is built, from
the document seed (`<select>` and its relatives). Unchosen branches are never
built: they have no cells and no components.

**Reactive choice** — A choice that a cell decides and that can change on any
tick (`<conditionalContent>`). Its active branch can change without the
document's names changing meaning. A branch that becomes active again returns
with the state it was left in. Whether the core keeps every branch built or
rebuilds on a change is the core's call, invisible to the author.

**Choice cell** — The cell holding the index of a reactive choice's active
branch: the first branch whose condition holds. It is structural when the
core rebuilds on a change, and an ordinary derived cell otherwise.

**Branch interface** — The names a choice exposes to the rest of the
document: those that every branch declares, each with the same component
type in every branch. Everything else in a branch is private to it. The build
checks the interface; `$cc.x` is legal only if `x` is in it. A choice whose
content feeds a typed parent (a `<math>`, a `<function>`) must yield that
same type from every branch.
_Avoid_: mirrored branches (the interface is the rule; branches may otherwise
differ freely)

**Document seed** — The one random seed a document is loaded with. Each
load-time choice draws from its own stream derived from the seed and the
choice's essential key, so a choice draws the same way wherever it is
rebuilt.
