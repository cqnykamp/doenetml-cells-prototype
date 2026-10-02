import { useRef } from "react";
import { useCell, useStore } from "../hooks";
import { toGraph, toPixel, useGraph } from "./Graph";

export function Point({ idx, inGraph }: { idx: number; inGraph: boolean }) {
  const store = useStore();
  const c = store.manifest.components[idx];
  const x = useCell(c.props.x);
  const y = useCell(c.props.y);
  if (!inGraph) {
    return <span data-comp={idx} data-name={c.name ?? undefined}>({fmt(x)}, {fmt(y)})</span>;
  }
  return <GraphPoint idx={idx} x={x} y={y} />;
}

function GraphPoint({ idx, x, y }: { idx: number; x: number; y: number }) {
  const store = useStore();
  const c = store.manifest.components[idx];
  const frame = useGraph();
  const dragging = useRef(false);
  const [px, py] = toPixel(frame, x, y);

  const move = (e: React.PointerEvent<SVGCircleElement>) => {
    if (!dragging.current) return;
    const rect = frame.svg.current!.getBoundingClientRect();
    const [gx, gy] = toGraph(frame, e.clientX - rect.left, e.clientY - rect.top);
    store.request([[c.props.x, gx], [c.props.y, gy]]);
  };
  return (
    <circle
      cx={px}
      cy={py}
      r={6}
      fill="#1f77b4"
      stroke="#fff"
      data-comp={idx}
      data-name={c.name ?? undefined}
      style={{ cursor: "grab" }}
      onPointerDown={(e) => {
        dragging.current = true;
        e.currentTarget.setPointerCapture(e.pointerId);
      }}
      onPointerUp={(e) => {
        dragging.current = false;
        e.currentTarget.releasePointerCapture(e.pointerId);
      }}
      onPointerMove={move}
    />
  );
}

export function fmt(v: number): string {
  if (Number.isNaN(v)) return "NaN";
  return Number.isInteger(v) ? String(v) : v.toPrecision(4);
}
