// Per-user UI preferences in localStorage. Storage may be unavailable or
// throw (private mode, cleared data): every access falls back to defaults.

import { useCallback, useState } from "react";

function read<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw === null ? fallback : (JSON.parse(raw) as T);
  } catch {
    return fallback;
  }
}

function write<T>(key: string, value: T): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Preferences are a convenience: ignore storage failures.
  }
}

/** `useState` persisted under `sfm.<key>`. */
export function usePersistentState<T>(key: string, fallback: T): [T, (value: T) => void] {
  const storageKey = `sfm.${key}`;
  const [value, setValue] = useState<T>(() => read(storageKey, fallback));
  const update = useCallback(
    (next: T) => {
      setValue(next);
      write(storageKey, next);
    },
    [storageKey],
  );
  return [value, update];
}
