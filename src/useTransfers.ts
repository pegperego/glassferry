import { useCallback, useEffect, useRef, useState } from "react";
import {
  isFinished,
  onTransferConflict,
  onTransferUpdate,
  transferCancel,
  transferClearFinished,
  transferConflictRespond,
  transferList,
  transferStart,
  type ConflictAction,
  type ConflictRequest,
  type Direction,
  type JobInfo,
} from "./api";

export interface Transfers {
  jobs: JobInfo[];
  activeCount: number;
  conflicts: ConflictRequest[];
  start: (sessionId: number, direction: Direction, srcDir: string, names: string[], dstDir: string) => Promise<void>;
  cancel: (jobId: number) => void;
  clearFinished: () => void;
  answerConflict: (action: ConflictAction, applyToAll: boolean) => void;
}

/** `onFinished` is called once per job when it reaches a final state. */
export function useTransfers(onFinished: (job: JobInfo) => void): Transfers {
  const [jobs, setJobs] = useState<Map<number, JobInfo>>(new Map());
  const [conflicts, setConflicts] = useState<ConflictRequest[]>([]);
  const finishedRef = useRef(onFinished);
  finishedRef.current = onFinished;
  const reported = useRef(new Set<number>());

  const apply = useCallback((updates: JobInfo[]) => {
    setJobs((current) => {
      const next = new Map(current);
      for (const job of updates) next.set(job.id, job);
      return next;
    });
    const finishedIds = new Set(updates.filter(isFinished).map((j) => j.id));
    if (finishedIds.size > 0) setConflicts((queue) => queue.filter((c) => !finishedIds.has(c.jobId)));
    for (const job of updates) {
      if (isFinished(job) && !reported.current.has(job.id)) {
        reported.current.add(job.id);
        finishedRef.current(job);
      }
    }
  }, []);

  useEffect(() => {
    const unlistenUpdates = onTransferUpdate(apply);
    const unlistenConflicts = onTransferConflict((request) =>
      setConflicts((queue) => (queue.some((c) => c.id === request.id) ? queue : [...queue, request])),
    );
    void transferList().then((list) => {
      // Jobs already finished before this view existed are not "new".
      for (const job of list) if (isFinished(job)) reported.current.add(job.id);
      apply(list);
    });
    return () => {
      void unlistenUpdates.then((fn) => fn());
      void unlistenConflicts.then((fn) => fn());
    };
  }, [apply]);

  const start = useCallback(
    async (sessionId: number, direction: Direction, srcDir: string, names: string[], dstDir: string) => {
      await transferStart(sessionId, direction, srcDir, names, dstDir);
    },
    [],
  );

  const clearFinished = useCallback(() => {
    void transferClearFinished();
    setJobs((current) => new Map([...current].filter(([, job]) => !isFinished(job))));
  }, []);

  const answerConflict = useCallback(
    (action: ConflictAction, applyToAll: boolean) => {
      const [current, ...rest] = conflicts;
      if (!current) return;
      // "Apply to all" also answers the other prompts queued for the same job.
      const remaining = applyToAll || action === "cancel" ? rest.filter((c) => c.jobId !== current.jobId) : rest;
      for (const other of rest) {
        if (!remaining.includes(other)) void transferConflictRespond(other.id, action, applyToAll);
      }
      setConflicts(remaining);
      void transferConflictRespond(current.id, action, applyToAll);
    },
    [conflicts],
  );

  const list = [...jobs.values()].sort((a, b) => a.id - b.id);
  return {
    jobs: list,
    activeCount: list.filter((j) => !isFinished(j)).length,
    conflicts,
    start,
    cancel: (jobId: number) => void transferCancel(jobId),
    clearFinished,
    answerConflict,
  };
}
