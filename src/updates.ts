// Rules of the update checks (pure, tested): when to check on its own, which
// versions to show, what a restart would interrupt.

import type { EditInfo, JobInfo } from "./api";

/** Automatic checks: at start, then once a day while the app stays open. */
export const CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;
/** Delay of the first automatic check, so it does not compete with the start. */
export const FIRST_CHECK_DELAY_MS = 5_000;

/**
 * Whether to offer `version`: a manual check always shows it, an automatic
 * one skips the version the user chose to skip.
 */
export function shouldOffer(version: string, skipped: string | null, manual: boolean): boolean {
  return manual || version !== skipped;
}

const ACTIVE_JOBS: JobInfo["state"][] = ["scanning", "running", "waitingConflict"];

export interface RestartImpact {
  /** Running transfers: interrupted, they can be resumed after the restart. */
  transfers: JobInfo[];
  /** Open files with changes not uploaded yet: kept and reattached on the next connect. */
  edits: EditInfo[];
}

/** What a restart now would interrupt. */
export function restartImpact(jobs: JobInfo[], edits: EditInfo[]): RestartImpact {
  return {
    transfers: jobs.filter((job) => ACTIVE_JOBS.includes(job.state)),
    edits: edits.filter((edit) => edit.state !== "synced"),
  };
}

/** Download percentage; null while the size is unknown. */
export function percentOf(downloaded: number, total: number | null): number | null {
  if (!total) return null;
  return Math.min(100, Math.round((downloaded / total) * 100));
}
