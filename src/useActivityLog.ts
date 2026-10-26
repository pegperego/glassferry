import { useCallback, useEffect, useRef, useState } from "react";
import { logClear, logSetCapacity, logSnapshot, onLogBatch, type LogEntry } from "./api";
import { DEFAULT_LOG_CAPACITY, clampCapacity, mergeBatch } from "./logStore";
import { usePersistentState } from "./settings";

export interface ActivityLog {
  entries: LogEntry[];
  capacity: number;
  setCapacity: (capacity: number) => void;
  clear: () => void;
  /** Errors received while the panel was closed. */
  unseenErrors: number;
  markSeen: () => void;
}

export function useActivityLog(panelOpen: boolean): ActivityLog {
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [storedCapacity, setStoredCapacity] = usePersistentState("log.capacity", DEFAULT_LOG_CAPACITY);
  const capacity = clampCapacity(storedCapacity);
  const [unseenErrors, setUnseenErrors] = useState(0);
  const capacityRef = useRef(capacity);
  const openRef = useRef(panelOpen);
  capacityRef.current = capacity;
  openRef.current = panelOpen;

  useEffect(() => {
    void logSetCapacity(capacity);
    setEntries((current) => (current.length > capacity ? current.slice(current.length - capacity) : current));
  }, [capacity]);

  useEffect(() => {
    let disposed = false;
    // Subscribe first, then load the backlog: batches received meanwhile are
    // newer and are applied on top of the snapshot.
    const unlisten = onLogBatch((batch) => {
      setEntries((current) => mergeBatch(current, batch, capacityRef.current));
      if (!openRef.current) {
        const errors = batch.filter((e) => e.status === "error").length;
        if (errors > 0) setUnseenErrors((n) => n + errors);
      }
    });
    void logSnapshot().then((snapshot) => {
      if (!disposed) setEntries((current) => mergeBatch(snapshot, current, capacityRef.current));
    });
    return () => {
      disposed = true;
      void unlisten.then((fn) => fn());
    };
  }, []);

  const clear = useCallback(() => {
    setEntries([]);
    setUnseenErrors(0);
    void logClear();
  }, []);

  const markSeen = useCallback(() => setUnseenErrors(0), []);

  return {
    entries,
    capacity,
    setCapacity: (value: number) => setStoredCapacity(clampCapacity(value)),
    clear,
    unseenErrors,
    markSeen,
  };
}
