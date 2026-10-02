import { useCell, useStore } from "../hooks";
import { fmt } from "./Point";

export function NumberView({ idx }: { idx: number }) {
  const store = useStore();
  const c = store.manifest.components[idx];
  const v = useCell(c.props.value);
  return <span data-comp={idx} data-name={c.name ?? undefined}>{fmt(v)}</span>;
}
