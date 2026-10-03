import { useEffect, useState } from "react";
import { CellStore, loadDocument, type BackendKind, type LoadTimings } from "./core";
import { StoreContext } from "./hooks";
import { CommitReporter, Component } from "./renderers";
import { parseDoenetML } from "./parse";

const FIXTURES = [
  "points-10", "points-100", "points-1000", "points-10000", "points-50000",
  "chain-10", "chain-100", "chain-1000", "chain-10000", "chain-100000",
  "fanout-10", "fanout-100", "fanout-1000", "fanout-10000",
  "aliases-1000", "grid-100x10", "grid-1000x10", "grid-1000x100",
];

const SAMPLE = `<graph name="g">
  <point name="p1" x="1" y="2"/>
  <point name="p2" x="$n" y="$p1.y"/>
  <op name="neg" kind="negate" args="$p1.x"/>
  <point name="p3" x="$neg" y="-3"/>
</graph>
<p>n = <numberInput name="n" value="4"/>, p1 = $p1, p2.x = $p2.x, p3 = $p3</p>`;

declare global {
  interface Window {
    __cells?: {
      store: CellStore | null;
      timings: Partial<LoadTimings> | null;
      ready: boolean;
      load: (spec: string) => Promise<void>;
    };
  }
}

export function App() {
  const params = new URLSearchParams(location.search);
  const [store, setStore] = useState<CellStore | null>(null);
  const [timings, setTimings] = useState<Partial<LoadTimings> | null>(null);
  const [source, setSource] = useState(SAMPLE);
  const [error, setError] = useState<string | null>(null);
  const [evaluator, setEvaluator] = useState(params.get("eval") ?? "dirty-closure");
  const [backend, setBackend] = useState<BackendKind>((params.get("backend") as BackendKind) ?? "main");

  async function loadJson(json: string, t: Partial<LoadTimings>) {
    setError(null);
    window.__cells!.ready = false;
    try {
      const s = await loadDocument(json, backend, evaluator, t);
      const t0 = performance.now();
      t.firstRender = NaN;
      setStore(s);
      setTimings(t);
      // React schedules the commit as a task, so wait until it has happened.
      const waitCommit = () => {
        if (s.firstCommitAt === null) {
          requestAnimationFrame(waitCommit);
          return;
        }
        t.firstRender = s.firstCommitAt - t0;
        setTimings({ ...t });
        window.__cells!.timings = t;
        window.__cells!.store = s;
        window.__cells!.ready = true;
      };
      requestAnimationFrame(waitCommit);
    } catch (e) {
      setError(String(e));
    }
  }

  async function loadFixture(spec: string) {
    const t: Partial<LoadTimings> = {};
    const t0 = performance.now();
    const json = await (await fetch(`/${spec}.json`)).text();
    t.fetch = performance.now() - t0;
    await loadJson(json, t);
  }

  async function loadSource() {
    const t: Partial<LoadTimings> = {};
    const t0 = performance.now();
    const json = await parseDoenetML(source.replace(/<\/?p>/g, ""));
    t.fetch = performance.now() - t0; // parse time stands in for fetch here
    await loadJson(json, t);
  }

  useEffect(() => {
    window.__cells = { store: null, timings: null, ready: false, load: loadFixture };
    const doc = params.get("doc");
    if (doc) loadFixture(doc);
    else loadSource();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div>
      <div className="controls">
        <label>
          Fixture:{" "}
          <select defaultValue={params.get("doc") ?? ""} onChange={(e) => e.target.value && loadFixture(e.target.value)}>
            <option value="">(source below)</option>
            {FIXTURES.map((f) => <option key={f} value={f}>{f}</option>)}
          </select>
        </label>
        <label>
          Evaluator:{" "}
          <select value={evaluator} onChange={(e) => { setEvaluator(e.target.value); store?.setEvaluator(e.target.value); }}>
            <option>dirty-closure</option>
            <option>dirty-scan</option>
            <option>full</option>
          </select>
        </label>
        <label>
          Backend:{" "}
          <select value={backend} onChange={(e) => setBackend(e.target.value as BackendKind)}>
            <option value="main">main thread</option>
            <option value="worker-sab">worker + SharedArrayBuffer</option>
            <option value="worker-msg">worker + postMessage</option>
          </select>
        </label>
        <button onClick={loadSource}>Load source</button>
        <textarea value={source} onChange={(e) => setSource(e.target.value)} />
      </div>
      {error && <div style={{ color: "crimson" }}>{error}</div>}
      {timings && <Timings t={timings} store={store} />}
      {store && (
        <StoreContext.Provider value={store}>
          <CommitReporter />
          <Component idx={store.manifest.root} inGraph={false} />
        </StoreContext.Provider>
      )}
    </div>
  );
}

function Timings({ t, store }: { t: Partial<LoadTimings>; store: CellStore | null }) {
  const c = t.core;
  const f = (v?: number) => (v === undefined || Number.isNaN(v) ? "   …" : v.toFixed(2).padStart(8));
  return (
    <div className="timings">
      {`cells ${store?.manifest.nCells ?? "?"}  essential ${store?.manifest.nEssential ?? "?"}  components ${store?.manifest.components.length ?? "?"}\n`}
      {`backend ${t.backend}  fetch/parse ${f(t.fetch)}  worker ${f(t.workerSpawn)}  wasm init ${f(t.wasmInit)}  core total ${f(t.coreTotal)}  manifest ${f(t.manifest)} + ${f(t.manifestTransfer)}  first render ${f(t.firstRender)}  (ms)\n`}
      {c && `  core: deserialize ${f(c.deserialize)}  build ${f(c.build)}  schedule ${f(c.schedule)}  initial compute ${f(c.initial_compute)}`}
    </div>
  );
}
