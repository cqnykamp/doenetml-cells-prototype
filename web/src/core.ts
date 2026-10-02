// The renderer-side counterpart of the cell-addressed interface (ADR 0001):
// a zero-copy Float64Array view over the core's cells, per-cell-index
// subscriptions, and cell-addressed requests.
import init, { Core } from "./wasm/cells_wasm.js";

export interface ManifestChild { c?: number; t?: string }
export interface ManifestComponent {
  kind: string;
  name: string | null;
  parent: number | null;
  props: Record<string, number>;
  children: ManifestChild[];
}
export interface Manifest {
  root: number;
  nEssential: number;
  nCells: number;
  components: ManifestComponent[];
}

export interface LoadTimings {
  fetch: number;
  wasmInit: number;
  core: { deserialize: number; build: number; schedule: number; initial_compute: number };
  coreTotal: number;
  manifest: number;
  firstRender: number;
}

export interface TickSample {
  /** ms spent inside Core.request (inversion + recompute) */
  core: number;
  /** ms from request start until the React commit for that request */
  commit: number;
  /** ms from request start until the next animation frame */
  frame: number;
  changed: number;
}

type Listener = () => void;

let wasmMemory: WebAssembly.Memory | null = null;

export class CellStore {
  core: Core;
  manifest: Manifest;
  private view: Float64Array;
  private listeners: (Set<Listener> | undefined)[];
  private anyListeners = new Set<Listener>();
  samples: TickSample[] = [];
  /** Incremented on every request; lets a component re-render per tick. */
  version = 0;
  /** performance.now() of the first React commit after construction. */
  firstCommitAt: number | null = null;
  private pendingCommit: { start: number; sample: TickSample } | null = null;

  constructor(core: Core, manifest: Manifest) {
    this.core = core;
    this.manifest = manifest;
    this.view = this.makeView();
    this.listeners = new Array(manifest.nCells);
  }

  private makeView(): Float64Array {
    return new Float64Array(wasmMemory!.buffer, this.core.cells_ptr(), this.core.cells_len());
  }

  /** The live cell array. Re-derived if wasm memory grew since last call. */
  cells(): Float64Array {
    if (this.view.buffer !== wasmMemory!.buffer || this.view.byteOffset !== this.core.cells_ptr()) {
      this.view = this.makeView();
    }
    return this.view;
  }

  get(cell: number): number {
    return this.cells()[cell];
  }

  subscribe(cell: number, fn: Listener): () => void {
    (this.listeners[cell] ??= new Set()).add(fn);
    return () => this.listeners[cell]?.delete(fn);
  }

  subscribeAny(fn: Listener): () => void {
    this.anyListeners.add(fn);
    return () => this.anyListeners.delete(fn);
  }

  /** Cell-addressed write. Returns the changed cell indices. */
  request(pairs: [number, number][]): Uint32Array {
    const start = performance.now();
    const cells = new Uint32Array(pairs.map((p) => p[0]));
    const values = new Float64Array(pairs.map((p) => p[1]));
    const changed = this.core.request(cells, values);
    const afterCore = performance.now();
    const sample: TickSample = { core: afterCore - start, commit: NaN, frame: NaN, changed: changed.length };
    this.samples.push(sample);
    this.pendingCommit = { start, sample };
    this.version++;
    requestAnimationFrame((_t) => {
      sample.frame = performance.now() - start;
    });
    for (let i = 0; i < changed.length; i++) {
      const set = this.listeners[changed[i]];
      if (set) for (const fn of set) fn();
    }
    for (const fn of this.anyListeners) fn();
    return changed;
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

export async function loadDocument(dastJson: string, timings: Partial<LoadTimings>): Promise<CellStore> {
  let t = performance.now();
  if (!wasmMemory) {
    const mod = await init();
    wasmMemory = mod.memory;
  }
  timings.wasmInit = performance.now() - t;

  t = performance.now();
  const core = new Core(dastJson);
  timings.coreTotal = performance.now() - t;
  timings.core = JSON.parse(core.load_timings_json());

  t = performance.now();
  const manifest: Manifest = JSON.parse(core.manifest_json());
  timings.manifest = performance.now() - t;
  return new CellStore(core, manifest);
}
