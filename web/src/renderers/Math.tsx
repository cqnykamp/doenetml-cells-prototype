import { useCell, useStore } from "../hooks";
import { fmt } from "./Point";

/** `<math>`: shows the numeric value when the expression lowered to a
 * number, else the expression text from the arena (main-thread backend
 * only; workers show the handle). */
export function MathView({ idx }: { idx: number }) {
  const store = useStore();
  const value = useCell(store.comps.cell(idx, "value"));
  const handle = useCell(store.comps.cell(idx, "expr"));
  const text = Number.isNaN(value) ? (store.exprText(handle) ?? `expr#${handle}`) : fmt(value);
  return <span className="math" data-comp={idx} data-name={store.comps.name(idx) ?? undefined}>{text}</span>;
}
