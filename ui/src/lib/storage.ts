// localStorage that never throws (private windows, blocked storage, CEF without a
// persistent profile). Values are JSON.

const PREFIX = 'bmcp.';

export function load<T>(key: string, fallback: T): T {
  try {
    const raw = window.localStorage.getItem(PREFIX + key);
    return raw === null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback;
  }
}

export function save(key: string, value: unknown): void {
  try {
    window.localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // Storage unavailable: settings simply do not persist.
  }
}

/** Calls back when another page of the same origin (e.g. the new tab page) changes a key. */
export function watch(key: string, cb: () => void): () => void {
  const handler = (e: StorageEvent) => {
    if (e.key === PREFIX + key) cb();
  };
  window.addEventListener('storage', handler);
  return () => window.removeEventListener('storage', handler);
}
