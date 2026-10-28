import { useCallback, useEffect, useRef, useState } from "react";
import {
  editClose,
  editConfirmSave,
  editList,
  editOpen,
  editReopen,
  editResolve,
  editReveal,
  onEditConfirm,
  onEditConflict,
  onEditUpdate,
  type EditChoice,
  type EditConfirm,
  type EditConflict,
  type EditInfo,
} from "./api";
import { usePersistentState } from "./settings";

export interface Edits {
  edits: EditInfo[];
  /** Files with local changes not on the server. */
  unsyncedCount: number;
  conflicts: EditConflict[];
  /** Upload confirmations pending (protected connections). */
  confirms: EditConfirm[];
  /** `confirmSaves`: protected connection; skipped for files the user said not to ask again. */
  open: (sessionId: number, path: string, confirmSaves: boolean, connectionId: string) => Promise<void>;
  /** `remember`: never ask again for this connection and path. */
  answerConfirm: (id: number, allow: boolean, remember: boolean) => void;
  /** Uploads changes left unconfirmed. */
  upload: (id: number) => void;
  close: (id: number) => void;
  reopen: (id: number) => void;
  reveal: (id: number) => void;
  resolve: (id: number, choice: EditChoice) => void;
  /** Reloads the list after a disconnect: the backend closed or detached that session's files. */
  refresh: () => void;
}

export function useEdits(onError: (message: string) => void): Edits {
  const [edits, setEdits] = useState<Map<number, EditInfo>>(new Map());
  const [conflicts, setConflicts] = useState<EditConflict[]>([]);
  const [confirms, setConfirms] = useState<EditConfirm[]>([]);
  /** "connectionId:path" of files whose uploads are not confirmed any more. */
  const [noConfirm, setNoConfirm] = usePersistentState<string[]>("edits.noConfirm", []);
  /** Edit id → "connectionId:path", to remember an answer. */
  const keys = useRef(new Map<number, string>());

  const upsert = useCallback((info: EditInfo) => {
    setEdits((current) => new Map(current).set(info.id, info));
  }, []);

  useEffect(() => {
    const unlistenUpdate = onEditUpdate(upsert);
    const unlistenConflict = onEditConflict((conflict) =>
      setConflicts((queue) => (queue.some((c) => c.id === conflict.id) ? queue : [...queue, conflict])),
    );
    const unlistenConfirm = onEditConfirm((confirm) =>
      setConfirms((queue) => (queue.some((c) => c.id === confirm.id) ? queue : [...queue, confirm])),
    );
    void editList().then((list) => list.forEach(upsert));
    return () => {
      void unlistenUpdate.then((fn) => fn());
      void unlistenConflict.then((fn) => fn());
      void unlistenConfirm.then((fn) => fn());
    };
  }, [upsert]);

  const report = (promise: Promise<unknown>) => void promise.catch((e) => onError(String(e)));

  const list = [...edits.values()].sort((a, b) => a.id - b.id);
  return {
    edits: list,
    unsyncedCount: list.filter((e) => e.state !== "synced").length,
    conflicts,
    confirms,
    open: async (sessionId, path, confirmSaves, connectionId) => {
      const key = `${connectionId}:${path}`;
      try {
        const info = await editOpen(sessionId, path, confirmSaves && !noConfirm.includes(key));
        keys.current.set(info.id, key);
        upsert(info);
      } catch (e) {
        onError(String(e));
      }
    },
    answerConfirm: (id, allow, remember) => {
      setConfirms((queue) => queue.filter((c) => c.id !== id));
      const key = keys.current.get(id);
      if (allow && remember && key && !noConfirm.includes(key)) setNoConfirm([...noConfirm, key]);
      report(editConfirmSave(id, allow));
    },
    upload: (id) => report(editConfirmSave(id, true)),
    close: (id) => {
      report(editClose(id));
      setEdits((current) => {
        const next = new Map(current);
        next.delete(id);
        return next;
      });
      setConflicts((queue) => queue.filter((c) => c.id !== id));
      setConfirms((queue) => queue.filter((c) => c.id !== id));
    },
    reopen: (id) => report(editReopen(id)),
    reveal: (id) => report(editReveal(id)),
    resolve: (id, choice) => {
      setConflicts((queue) => queue.filter((c) => c.id !== id));
      report(editResolve(id, choice));
    },
    refresh: () =>
      report(
        editList().then((list) => {
          setEdits(new Map(list.map((e) => [e.id, e])));
          // Prompts of detached files were answered "not now" by the backend.
          const attached = new Set(list.filter((e) => e.state !== "detached").map((e) => e.id));
          setConflicts((queue) => queue.filter((c) => attached.has(c.id)));
          setConfirms((queue) => queue.filter((c) => attached.has(c.id)));
        }),
      ),
  };
}
