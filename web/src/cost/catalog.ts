import { useSyncExternalStore } from "react";
import bundled from "./price-history.json";
import { sources } from "./pricing.json";
import type { PriceHistory } from "./estimate";

const feed = "https://raw.githubusercontent.com/isamisushi/kikimimi/main/web/src/cost/price-history.json";
const key = "kikimimi.pricing.history.v1";
let history: PriceHistory = bundled;
const listeners = new Set<() => void>();

function accept(next: PriceHistory) {
  if (!(Date.parse(next.fetchedAt) > Date.parse(history.fetchedAt)) ||
      next.versions.length < history.versions.length) return;
  for (const [index, version] of next.versions.entries()) {
    if (index < history.versions.length) {
      if (JSON.stringify(version) !== JSON.stringify(history.versions[index])) return;
      continue;
    }
    if (!/^\d{4}-\d{2}-\d{2}$/.test(version.effectiveFrom) ||
        version.effectiveFrom <= next.versions[index - 1].effectiveFrom ||
        version.effectiveFrom > next.fetchedAt.slice(0, 10) ||
        !version.models.length) return;
    for (const model of version.models) {
      if (!model.id || typeof model.name !== "string" || !["openai","anthropic"].includes(model.provider) ||
          !model.variants.length || !model.variants.every(v =>
            [v.input, v.output, v.read ?? 0, v.write ?? 0].every(n => Number.isFinite(n) && n >= 0)
            && v.read !== undefined && v.write !== undefined)) return;
    }
  }
  history = next;
  listeners.forEach(fn => fn());
}
let interval: ReturnType<typeof setInterval>;
async function refresh() {
  try {
    const response = await fetch(feed, { credentials: "omit", referrerPolicy: "no-referrer", signal: AbortSignal.timeout(15000) });
    if (response.ok) {
      accept(await response.json());
      localStorage.setItem(key, JSON.stringify(history));
    }
  } catch { /* The bundled or cached history remains available offline. */ }
}
function subscribe(listener: () => void) {
  listeners.add(listener);
  if (listeners.size === 1) {
    try { accept(JSON.parse(localStorage.getItem(key) ?? "null")); } catch { /* empty or invalid cache */ }
    void refresh();
    interval = setInterval(refresh, 6 * 3600000);
  }
  return () => { listeners.delete(listener); if (!listeners.size) clearInterval(interval); };
}
export function useCatalog() {
  const data = useSyncExternalStore(subscribe, () => history);
  return { history: data, fetchedAt: data.fetchedAt, models: data.versions[data.versions.length - 1].models, sources };
}
