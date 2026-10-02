import { useLayoutEffect, useSyncExternalStore } from "react";
import { useStore } from "../hooks";
import type { ManifestChild } from "../core";
import { Graph } from "./Graph";
import { Point } from "./Point";
import { NumberView } from "./Number";
import { NumberInput } from "./NumberInput";

export function Component({ idx, inGraph }: { idx: number; inGraph: boolean }) {
  const store = useStore();
  const c = store.manifest.components[idx];
  switch (c.kind) {
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
  const c = store.manifest.components[idx];
  return <ChildRange items={c.children} from={0} to={c.children.length} inGraph={inGraph} />;
}

function ChildRange({ items, from, to, inGraph }: { items: ManifestChild[]; from: number; to: number; inGraph: boolean }) {
  const n = to - from;
  if (n <= CHUNK) {
    return (
      <>
        {items.slice(from, to).map((ch, i) =>
          ch.c !== undefined ? <Component key={from + i} idx={ch.c} inGraph={inGraph} /> : <span key={from + i}>{ch.t}</span>,
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
