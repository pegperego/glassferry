import { useCallback, useEffect, useState } from "react";
import { connectionsLoad, connectionsSave, type Store } from "./api";

export interface Connections {
  /** null while loading. */
  store: Store | null;
  /** Load problem (unreadable file set aside), shown once. */
  warning: string | null;
  dismissWarning: () => void;
  /** Saves the whole store; rejects with the backend's validation error. */
  save: (store: Store) => Promise<Store>;
}

export function useConnections(): Connections {
  const [store, setStore] = useState<Store | null>(null);
  const [warning, setWarning] = useState<string | null>(null);

  useEffect(() => {
    connectionsLoad().then(
      (result) => {
        setStore(result.store);
        setWarning(result.warning);
      },
      (e) => setWarning(String(e)),
    );
  }, []);

  const save = useCallback(async (next: Store) => {
    const saved = await connectionsSave(next);
    setStore(saved);
    return saved;
  }, []);

  return { store, warning, dismissWarning: () => setWarning(null), save };
}
