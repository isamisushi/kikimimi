import { useSyncExternalStore } from "react";
export type ActivityFilter = { device: string; repository: string };
const key = "kikimimi.activity-filter.v1";
const empty: ActivityFilter = { device: "all", repository: "all" };
function restore(): ActivityFilter {
  try {
    const value = JSON.parse(sessionStorage.getItem(key) || "null");
    if (typeof value?.device === "string" && typeof value?.repository === "string") return value;
  } catch { /* Session storage is optional. */ }
  return empty;
}
let filter = restore();
const listeners = new Set<() => void>();
export function setActivityFilter(next: ActivityFilter) {
  filter = next;
  try { sessionStorage.setItem(key, JSON.stringify(next)); } catch { /* Optional. */ }
  listeners.forEach((listener) => listener());
}
function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}
export function useActivityFilter() {
  return useSyncExternalStore(subscribe, () => filter);
}
export function activityFilterPath(path: string): string {
  if (!path.startsWith("/web/q/") || path.split("?")[0] === "/web/q/filter-options") return path;
  const [route, search = ""] = path.split("?");
  const params = new URLSearchParams(search);
  if (filter.device !== "all") params.set("device", filter.device);
  if (filter.repository === "none") params.set("without_repository", "true");
  else if (filter.repository.startsWith("repo:")) params.set("repository", filter.repository.slice(5));
  const query = params.toString();
  return query ? `${route}?${query}` : route;
}
