import { useEffect, useSyncExternalStore } from "react";
import ja from "./ja.json";

export type Locale = "en" | "ja";
const STORAGE_KEY = "kikimimi.ui-language.v1";
const listeners = new Set<() => void>();
function readLocale(): Locale {
  try {
    return localStorage.getItem(STORAGE_KEY) === "ja" ? "ja" : "en";
  } catch {
    return "en";
  }
}
let locale = readLocale();
export function getLocale(): Locale {
  return locale;
}
export function intlLocale(): string {
  return locale === "ja" ? "ja-JP" : "en-US";
}
export function setLocale(next: Locale) {
  if (next !== "en" && next !== "ja") return;
  try {
    localStorage.setItem(STORAGE_KEY, next);
  } catch {
    /* Still usable for this session. */
  }
  locale = next;
  document.documentElement.lang = next;
  listeners.forEach((listener) => listener());
}
function onStorage(event: StorageEvent) {
  if (event.key === STORAGE_KEY || event.key === null) {
    locale = readLocale();
    document.documentElement.lang = locale;
    listeners.forEach((listener) => listener());
  }
}
function subscribe(listener: () => void) {
  if (listeners.size === 0) window.addEventListener("storage", onStorage);
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) window.removeEventListener("storage", onStorage);
  };
}
export function useLocale(): Locale {
  const value = useSyncExternalStore(subscribe, getLocale, () => "en" as const);
  useEffect(() => {
    document.documentElement.lang = value;
  }, [value]);
  return value;
}
/** Source messages stay in English. Values are interpolated after translation. */
export function t(message: string, values: readonly unknown[] = []): string {
  const translated =
    locale === "ja"
      ? ((ja as Record<string, string>)[message] ?? message)
      : message;
  return translated.replace(/\{(\d+)\}/g, (token, index: string) =>
    Number(index) < values.length ? String(values[Number(index)]) : token,
  );
}
