import { useCallback, useEffect, useState } from "react";
import {
  editClose,
  editList,
  editOpen,
  editReopen,
  editResolve,
  editReveal,
  onEditConflict,
  onEditUpdate,
  type EditChoice,
  type EditConflict,
  type EditInfo,
} from "./api";

export interface Edits {
  edits: EditInfo[];
  /** Files with local changes not on the server. */
  unsyncedCount: number;
  conflicts: EditConflict[];
  open: (sessionId: number, path: string) => Promise<void>;
  close: (id: number) => void;
  reopen: (id: number) => void;
  reveal: (id: number) => void;
  resolve: (id: number, choice: EditChoice) => void;
  /** Forgets the edits of a session (after disconnect, the backend closed them). */
  dropSession: (sessionId: number) => void;
}

export function useEdits(onError: (message: string) => void): Edits {
  const [edits, setEdits] = useState<Map<number, EditInfo>>(new Map());
  const [conflicts, setConflicts] = useState<EditConflict[]>([]);

  const upsert = useCallback((info: EditInfo) => {
    setEdits((current) => new Map(current).set(info.id, info));
  }, []);

  useEffect(() => {
    const unlistenUpdate = onEditUpdate(upsert);
    const unlistenConflict = onEditConflict((conflict) =>
      setConflicts((queue) => (queue.some((c) => c.id === conflict.id) ? queue : [...queue, conflict])),
    );
    void editList().then((list) => list.forEach(upsert));
    return () => {
      void unlistenUpdate.then((fn) => fn());
      void unlistenConflict.then((fn) => fn());
    };
  }, [upsert]);

  const report = (promise: Promise<unknown>) => void promise.catch((e) => onError(String(e)));

  const list = [...edits.values()].sort((a, b) => a.id - b.id);
  return {
    edits: list,
    unsyncedCount: list.filter((e) => e.state !== "synced").length,
    conflicts,
    open: async (sessionId, path) => {
      try {
        upsert(await editOpen(sessionId, path));
      } catch (e) {
        onError(String(e));
      }
    },
    close: (id) => {
      report(editClose(id));
      setEdits((current) => {
        const next = new Map(current);
        next.delete(id);
        return next;
      });
      setConflicts((queue) => queue.filter((c) => c.id !== id));
    },
    reopen: (id) => report(editReopen(id)),
    reveal: (id) => report(editReveal(id)),
    resolve: (id, choice) => {
      setConflicts((queue) => queue.filter((c) => c.id !== id));
      report(editResolve(id, choice));
    },
    dropSession: (sessionId) => {
      setEdits((current) => new Map([...current].filter(([, e]) => e.sessionId !== sessionId)));
      setConflicts([]);
    },
  };
}
