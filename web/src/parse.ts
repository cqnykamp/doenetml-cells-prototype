// In-browser DoenetML parsing via the existing TypeScript parser, for the
// textarea in the UI. Benchmarks use pre-parsed fixtures instead.
export async function parseDoenetML(source: string): Promise<string> {
  const { lezerToDast, normalizeDocumentDast } = await import("@doenet/parser");
  const dast = normalizeDocumentDast(lezerToDast(source));
  return JSON.stringify(dast, (k, v) => (k === "position" || k === "sources" ? undefined : v));
}
