// The renderer-side counterpart of the cell-addressed interface (ADR 0001):
// a Float64Array view over the core's cells, per-cell-index subscriptions,
// and cell-addressed requests. Three backends differ only in where the core
// runs and how changed cells reach this thread:
//  - main:       wasm on the main thread; zero-copy view over wasm memory
//  - worker-sab: wasm in a worker; changed cells copied into a SharedArrayBuffer
//  - worker-msg: wasm in a worker; changed indices and values posted per tick
import init, { Core } from "./wasm/cells_wasm.js";
import type { FromWorker, ToWorker, WorkerMode } from "./worker";
import { ComponentTable, columnsFromCore } from "./components";

export type BackendKind = "main" | "worker-sab" | "worker-msg";

export interface LoadTimings {
  backend: BackendKind;
  fetch: number;
  /** worker spawn (worker backends only) */
  workerSpawn: number;
  wasmInit: number;
  core: { deserialize: number; build: number; schedule: number; initial_compute: number };
  coreTotal: number;
  /** building the component table views (main) or copying the columns out (worker) */
  manifest: number;
  /** constructing the ComponentTable on the main thread */
  manifestTransfer: number;
  firstRender: number;
}

export interface TickSample {
  /** ms inside Core.request (inversion + recompute), measured where the core runs */
  core: number;
  /** ms from request start until the changed cells are available on the main thread */
  roundTrip: number;
  /** ms from request start until the React commit for that request */
  commit: number;
  /** ms from request start until the next animation frame */
  frame: number;
  changed: number;
}

type Listener = () => void;

interface Backend {
  cells(): Float64Array;
  /** Resolves with changed indices once the main-thread cell view is up to date. */
  request(cells: Uint32Array, values: Float64Array, onCore: (ms: number) => void): Promise<Uint32Array> | Uint32Array;
  setEvaluator(name: string): void;
}

let wasmMemory: WebAssembly.Memory | null = null;

class MainBackend implements Backend {
  private view: Float64Array;
  constructor(private core: Core) {
    this.view = this.make();
  }
  private make() {
    return new Float64Array(wasmMemory!.buffer, this.core.cells_ptr(), this.core.cells_len());
  }
  cells() {
    if (this.view.buffer !== wasmMemory!.buffer || this.view.byteOffset !== this.core.cells_ptr()) this.view = this.make();
    return this.view;
  }
  request(cells: Uint32Array, values: Float64Array, onCore: (ms: number) => void) {
    const t = performance.now();
    const changed = this.core.request(cells, values);
    onCore(performance.now() - t);
    return changed;
  }
  setEvaluator(name: string) {
    this.core.set_evaluator(name);
  }
}

class WorkerBackend implements Backend {
  private view: Float64Array;
  private nextId = 1;
  private pending = new Map<number, { resolve: (c: Uint32Array) => void; onCore: (ms: number) => void }>();
  constructor(private worker: Worker, view: Float64Array) {
    this.view = view;
    worker.onmessage = (e: MessageEvent<FromWorker>) => {
      const m = e.data;
      if (m.type !== "tick") return;
      if (m.values) for (let i = 0; i < m.changed.length; i++) this.view[m.changed[i]] = m.values[i];
      const p = this.pending.get(m.id);
      if (p) {
        this.pending.delete(m.id);
        p.onCore(m.coreMs);
        p.resolve(m.changed);
      }
    };
  }
  cells() {
    return this.view;
  }
  request(cells: Uint32Array, values: Float64Array, onCore: (ms: number) => void) {
    const id = this.nextId++;
    return new Promise<Uint32Array>((resolve) => {
      this.pending.set(id, { resolve, onCore });
      this.worker.postMessage({ type: "request", id, cells, values } satisfies ToWorker, [cells.buffer, values.buffer]);
    });
  }
  setEvaluator(name: string) {
    this.worker.postMessage({ type: "setEvaluator", name } satisfies ToWorker);
  }
}

export class CellStore {
  comps: ComponentTable;
  backend: Backend;
  kind: BackendKind;
  private listeners: (Set<Listener> | undefined)[];
  private anyListeners = new Set<Listener>();
  samples: TickSample[] = [];
  /** Incremented on every applied tick; lets a component re-render per tick. */
  version = 0;
  /** performance.now() of the first React commit after construction. */
  firstCommitAt: number | null = null;
  private pendingCommit: { start: number; sample: TickSample } | null = null;

  constructor(backend: Backend, comps: ComponentTable, kind: BackendKind) {
    this.backend = backend;
    this.comps = comps;
    this.kind = kind;
    this.listeners = new Array(comps.nCells);
  }

  cells(): Float64Array {
    return this.backend.cells();
  }

  get(cell: number): number {
    return this.backend.cells()[cell];
  }

  subscribe(cell: number, fn: Listener): () => void {
    (this.listeners[cell] ??= new Set()).add(fn);
    return () => this.listeners[cell]?.delete(fn);
  }

  subscribeAny(fn: Listener): () => void {
    this.anyListeners.add(fn);
    return () => this.anyListeners.delete(fn);
  }

  setEvaluator(name: string) {
    this.backend.setEvaluator(name);
  }

  /** Cell-addressed write. Listeners fire once the changed cells are readable here. */
  request(pairs: [number, number][]): void {
    const start = performance.now();
    const cells = new Uint32Array(pairs.map((p) => p[0]));
    const values = new Float64Array(pairs.map((p) => p[1]));
    const sample: TickSample = { core: NaN, roundTrip: NaN, commit: NaN, frame: NaN, changed: 0 };
    this.samples.push(sample);
    const apply = (changed: Uint32Array) => {
      sample.roundTrip = performance.now() - start;
      sample.changed = changed.length;
      this.pendingCommit = { start, sample };
      this.version++;
      requestAnimationFrame(() => {
        sample.frame = performance.now() - start;
      });
      for (let i = 0; i < changed.length; i++) {
        const set = this.listeners[changed[i]];
        if (set) for (const fn of set) fn();
      }
      for (const fn of this.anyListeners) fn();
    };
    const r = this.backend.request(cells, values, (ms) => (sample.core = ms));
    if (r instanceof Promise) r.then(apply);
    else apply(r);
  }

  /** Called by the renderer after React commits a tick's updates. */
  markCommit() {
    this.firstCommitAt ??= performance.now();
    if (this.pendingCommit) {
      this.pendingCommit.sample.commit = performance.now() - this.pendingCommit.start;
      this.pendingCommit = null;
    }
  }
}

/** `bytes` is either DAST JSON (UTF-8) or the binary CDST wire format. */
export async function loadDocument(bytes: Uint8Array, kind: BackendKind, evaluator: string, timings: Partial<LoadTimings>): Promise<CellStore> {
  timings.backend = kind;
  if (kind === "main") {
    let t = performance.now();
    if (!wasmMemory) wasmMemory = (await init()).memory;
    timings.wasmInit = performance.now() - t;
    timings.workerSpawn = 0;
    t = performance.now();
    const core = new Core(bytes);
    core.set_evaluator(evaluator);
    timings.coreTotal = performance.now() - t;
    timings.core = JSON.parse(core.load_timings_json());
    t = performance.now();
    // Views over wasm memory: the component layer never grows after load,
    // but the cell array may be reallocated, so cells are re-viewed per read.
    const cols = columnsFromCore(core, wasmMemory, false);
    timings.manifest = performance.now() - t;
    t = performance.now();
    const comps = new ComponentTable(cols);
    timings.manifestTransfer = performance.now() - t;
    return new CellStore(new MainBackend(core), comps, kind);
  }

  const mode: WorkerMode = kind === "worker-sab" ? "sab" : "msg";
  if (mode === "sab" && typeof SharedArrayBuffer === "undefined") {
    throw new Error("SharedArrayBuffer unavailable: the page needs COOP/COEP headers");
  }
  let t = performance.now();
  const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
  const loaded = await new Promise<Extract<FromWorker, { type: "loaded" }>>((resolve, reject) => {
    worker.onmessage = (e: MessageEvent<FromWorker>) => {
      if (e.data.type === "loaded") resolve(e.data);
      else if (e.data.type === "error") reject(new Error(e.data.message));
    };
    worker.postMessage({ type: "load", bytes, mode, evaluator } satisfies ToWorker, [bytes.buffer as ArrayBuffer]);
  });
  const total = performance.now() - t;
  timings.wasmInit = loaded.wasmInit;
  timings.coreTotal = loaded.coreTotal;
  timings.core = JSON.parse(loaded.coreTimings);
  timings.manifest = loaded.columnsMs;
  // Everything in the round trip not accounted for by the worker's own stages:
  // spawning, module load, the document transfer in, and the columns out.
  timings.workerSpawn = total - loaded.wasmInit - loaded.coreTotal - loaded.columnsMs;
  t = performance.now();
  const comps = new ComponentTable(loaded.columns);
  timings.manifestTransfer = performance.now() - t;
  const view = loaded.sab ? new Float64Array(loaded.sab) : loaded.cells!;
  return new CellStore(new WorkerBackend(worker, view), comps, kind);
}
