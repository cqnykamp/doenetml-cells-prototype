import { useEffect, useState } from "react";
import { useCell, useStore } from "../hooks";

export function NumberInput({ idx }: { idx: number }) {
  const store = useStore();
  const c = store.manifest.components[idx];
  const v = useCell(c.props.value);
  const [text, setText] = useState(Number.isNaN(v) ? "" : String(v));
  useEffect(() => setText(Number.isNaN(v) ? "" : String(v)), [v]);
  const commit = () => {
    const n = text.trim() === "" ? NaN : Number(text);
    store.request([[c.props.value, n]]);
  };
  return (
    <input
      className="numberInput"
      type="text"
      inputMode="decimal"
      value={text}
      data-comp={idx}
      data-name={c.name ?? undefined}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => e.key === "Enter" && commit()}
    />
  );
}
