import { createContext, useContext, useSyncExternalStore } from "react";
import type { CellStore } from "./core";

export const StoreContext = createContext<CellStore | null>(null);

export function useStore(): CellStore {
  const s = useContext(StoreContext);
  if (!s) throw new Error("no store");
  return s;
}

/** Subscribe one component to one cell index. */
export function useCell(cell: number): number {
  const store = useStore();
  return useSyncExternalStore(
    (fn) => store.subscribe(cell, fn),
    () => store.get(cell),
  );
}
