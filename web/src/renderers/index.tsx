import { useLayoutEffect, useMemo, useSyncExternalStore } from "react";
import { useStore } from "../hooks";
import type { Child } from "../components";
import { Graph } from "./Graph";
import { Point } from "./Point";
import { NumberView } from "./Number";
import { NumberInput } from "./NumberInput";
import { Slider } from "./Slider";
import { BooleanInput } from "./BooleanInput";
import { MathView } from "./Math";
import { Circle, Line, Polygon } from "./Shapes";
import { CaseView, ChoiceView, TextView } from "./Choice";

export function Component({ idx, inGraph }: { idx: number; inGraph: boolean }) {
  const store = useStore();
  switch (store.comps.kind(idx)) {
    case "document":
      return <div className="doc"><Children idx={idx} inGraph={false} /></div>;
    case "graph":
      return <Graph idx={idx} />;
    case "point":
      return <Point idx={idx} inGraph={inGraph} />;
    case "number":
      return <NumberView idx={idx} />;
    case "numberInput":
      return <NumberInput idx={idx} />;
    case "slider":
      return <Slider idx={idx} />;
    case "booleanInput":
      return <BooleanInput idx={idx} />;
    case "math":
      return <MathView idx={idx} />;
    case "mathInput":
      return <NumberInput idx={idx} />;
    case "line":
      return inGraph ? <Line idx={idx} segment={false} /> : null;
    case "lineSegment":
      return inGraph ? <Line idx={idx} segment={true} /> : null;
    case "circle":
      return inGraph ? <Circle idx={idx} /> : null;
    case "polygon":
      return inGraph ? <Polygon idx={idx} /> : null;
    // A point list's children are points aliasing a shape's own points.
    case "pointList":
    case "p":
    case "group":
      return <Children idx={idx} inGraph={inGraph} />;
    case "text":
      return <TextView idx={idx} />;
    case "conditionalContent":
    case "select":
      return <ChoiceView idx={idx} inGraph={inGraph} />;
    case "case":
      return <CaseView idx={idx} inGraph={inGraph} />;
    case "setup":
      return null;
    case "evaluate":
      return <NumberView idx={idx} />;
    // A repeat's children are every iteration's expanded template; a
    // collect's are copies of what it gathered. Both render inline.
    case "repeatForSequence":
    case "collect":
      return <Children idx={idx} inGraph={inGraph} />;
    case "sequenceValue":
      return <NumberView idx={idx} />;
    default:
      return null; // <op> has no rendering
  }
}

/** React clones every child fiber of a parent on the update path, so a flat
 * list of N siblings costs O(N) per tick. Chunking children into a shallow
 * tree makes the update path O(CHUNK * log N) instead. */
const CHUNK = 32;

export function Children({ idx, inGraph }: { idx: number; inGraph: boolean }) {
  const store = useStore();
  // The table is replaced by a rebuild; `structureVersion` re-reads it.
  const items = useMemo(() => store.comps.children(idx), [store, store.comps, idx]);
  return <ChildRange items={items} from={0} to={items.length} inGraph={inGraph} />;
}

/** Children are keyed by stable component identity (DAST node and scope),
 * not by index, so after a rebuild React updates surviving components in
 * place and mounts only the iterations that appeared. */
function ChildRange({ items, from, to, inGraph }: { items: Child[]; from: number; to: number; inGraph: boolean }) {
  const store = useStore();
  const n = to - from;
  if (n <= CHUNK) {
    return (
      <>
        {items.slice(from, to).map((ch, i) =>
          "c" in ch ? <Component key={store.comps.key(ch.c, from + i)} idx={ch.c} inGraph={inGraph} /> : <span key={from + i}>{ch.t}</span>,
        )}
      </>
    );
  }
  // Split into CHUNK ranges of roughly equal size.
  const step = Math.ceil(n / CHUNK);
  const parts = [];
  for (let s = from; s < to; s += step) {
    parts.push(<ChildRange key={s} items={items} from={s} to={Math.min(to, s + step)} inGraph={inGraph} />);
  }
  return <>{parts}</>;
}

/** Reports React commits back to the store for commit-latency samples. */
export function CommitReporter() {
  const store = useStore();
  const version = useSyncExternalStore(
    (fn) => store.subscribeAny(fn),
    () => store.version,
  );
  useLayoutEffect(() => {
    store.markCommit();
  }, [store, version]);
  return null;
}
