// End-to-end measurement: load each fixture in headless Chromium, read the
// startup stage timings, then drag the target point and sample per-tick core
// time, React commit latency and frame latency. Writes results/raw/e2e-*.json.
import { test, expect, type Page } from "@playwright/test";
import { writeFileSync, mkdirSync, existsSync, readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

const SPECS = (process.env.CELLS_E2E_FIXTURES ?? "points-100,points-1000,points-10000,chain-1000,chain-10000,chain-100000,fanout-1000,fanout-10000,grid-1000x10,grid-1000x100").split(",");
const EVALUATORS = (process.env.CELLS_E2E_EVALS ?? "dirty-closure,full").split(",");
const BACKENDS = (process.env.CELLS_E2E_BACKENDS ?? "main,worker-sab,worker-msg").split(",");
const STEPS = 120;

function dragTarget(spec: string): string {
  const shape = spec.split("-")[0];
  if (shape === "chain" || shape === "aliases") return "p";
  return "p0";
}

async function waitReady(page: Page) {
  await page.waitForFunction(() => window.__cells?.ready === true, null, { timeout: 120_000 });
}

function stats(xs: number[]) {
  const s = xs.filter((x) => Number.isFinite(x)).sort((a, b) => a - b);
  if (s.length === 0) return { n: 0, mean: NaN, p50: NaN, p90: NaN, max: NaN };
  const q = (p: number) => s[Math.min(s.length - 1, Math.floor(p * s.length))];
  return { n: s.length, mean: s.reduce((a, b) => a + b, 0) / s.length, p50: q(0.5), p90: q(0.9), max: s[s.length - 1] };
}

const results: any[] = [];

test.afterAll(() => {
  const dir = resolve(here, "../../results/raw");
  mkdirSync(dir, { recursive: true });
  const stamp = new Date().toISOString().slice(0, 10);
  const file = resolve(dir, `e2e-${stamp}.json`);
  // Merge with the same day's earlier runs so partial sweeps accumulate.
  let merged = results;
  if (existsSync(file)) {
    const prev = JSON.parse(readFileSync(file, "utf8")).results as any[];
    const key = (r: any) => `${r.spec}|${r.backend ?? "main"}|${r.evaluator}`;
    const fresh = new Set(results.map(key));
    merged = [...prev.filter((r) => !fresh.has(key(r))), ...results];
  }
  writeFileSync(file, JSON.stringify({ date: stamp, steps: STEPS, results: merged }, null, 1));
});

for (const spec of SPECS) {
  for (const backend of BACKENDS) {
  for (const evaluator of EVALUATORS) {
    test(`${spec} [${backend}] [${evaluator}]`, async ({ page }) => {
      page.on("console", (m) => { if (m.type() === "error" || m.type() === "warning") console.log(`[browser ${m.type()}] ${m.text()}`); });
      page.on("pageerror", (e) => console.log(`[pageerror] ${e.message}`));
      await page.goto(`/?doc=${spec}&eval=${evaluator}&backend=${backend}`);
      await waitReady(page);
      const load = await page.evaluate(() => {
        const s = window.__cells!.store!;
        return { timings: window.__cells!.timings, nCells: s.manifest.nCells, nComponents: s.manifest.components.length };
      });

      const circle = page.locator(`circle[data-name="${dragTarget(spec)}"]`);
      await expect(circle).toHaveCount(1);
      await circle.scrollIntoViewIfNeeded();
      const box = (await circle.boundingBox())!;
      const cx = box.x + box.width / 2, cy = box.y + box.height / 2;
      await page.evaluate(() => { window.__cells!.store!.samples.length = 0; });
      await page.mouse.move(cx, cy);
      await page.mouse.down();
      for (let i = 1; i <= STEPS; i++) {
        const a = (i / STEPS) * Math.PI * 2;
        await page.mouse.move(cx + 120 * Math.sin(a), cy + 120 * Math.sin(2 * a));
      }
      await page.mouse.up();
      // Let the last frame callback land.
      await page.waitForTimeout(100);
      const samples = await page.evaluate(() => window.__cells!.store!.samples);
      // Chromium coalesces pointermove events when the main thread is busy,
      // so slow documents yield fewer samples than moves.
      expect(samples.length).toBeGreaterThan(10);

      const r = {
        spec, backend, evaluator, ...load,
        tick: {
          core: stats(samples.map((s: any) => s.core)),
          roundTrip: stats(samples.map((s: any) => s.roundTrip)),
          commit: stats(samples.map((s: any) => s.commit)),
          frame: stats(samples.map((s: any) => s.frame)),
          changed: stats(samples.map((s: any) => s.changed)),
        },
      };
      results.push(r);
      console.log(`${spec} [${backend}] [${evaluator}] cells=${load.nCells} core p50=${r.tick.core.p50.toFixed(3)}ms rt p50=${r.tick.roundTrip.p50.toFixed(2)}ms commit p50=${r.tick.commit.p50.toFixed(2)}ms frame p50=${r.tick.frame.p50.toFixed(2)}ms first render=${load.timings?.firstRender?.toFixed(1)}ms`);
    });
  }
  }
}
