// Pure helpers for the activity log list.

import type { LogChannel, LogEntry } from "./api";

export const DEFAULT_LOG_CAPACITY = 1000;
export const MIN_LOG_CAPACITY = 100;
export const MAX_LOG_CAPACITY = 50000;

/**
 * Applies a batch of new/updated entries (ids are increasing). Updates of
 * entries already evicted are dropped; the list is trimmed to `capacity`.
 */
export function mergeBatch(entries: LogEntry[], batch: LogEntry[], capacity: number): LogEntry[] {
  if (batch.length === 0) return entries;
  const next = entries.slice();
  const indexById = new Map(next.map((e, i) => [e.id, i]));
  const firstId = next.length > 0 ? next[0].id : -Infinity;
  for (const entry of batch) {
    const index = indexById.get(entry.id);
    if (index !== undefined) next[index] = entry;
    else if (entry.id > firstId || next.length === 0) {
      indexById.set(entry.id, next.length);
      next.push(entry);
    }
  }
  next.sort((a, b) => a.id - b.id);
  return next.length > capacity ? next.slice(next.length - capacity) : next;
}

export interface LogFilter {
  errorsOnly: boolean;
  host: string | null;
  channel: LogChannel | null;
  text: string;
}

export const emptyFilter: LogFilter = { errorsOnly: false, host: null, channel: null, text: "" };

export function filterEntries(entries: LogEntry[], filter: LogFilter): LogEntry[] {
  const needle = filter.text.trim().toLowerCase();
  return entries.filter(
    (e) =>
      (!filter.errorsOnly || e.status === "error") &&
      (filter.host === null || e.host === filter.host) &&
      (filter.channel === null || e.channel === filter.channel) &&
      (needle === "" ||
        [e.op, e.detail, e.error ?? "", e.extra ?? "", e.host ?? ""].some((v) => v.toLowerCase().includes(needle))),
  );
}

export function clampCapacity(value: number): number {
  if (!Number.isFinite(value)) return DEFAULT_LOG_CAPACITY;
  return Math.min(MAX_LOG_CAPACITY, Math.max(MIN_LOG_CAPACITY, Math.round(value)));
}

const pad = (n: number, width = 2) => String(n).padStart(width, "0");

/** `HH:MM:SS.mmm` in local time. */
export function formatLogTime(epochMs: number): string {
  const d = new Date(epochMs);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`;
}

export function formatDuration(ms: number | null): string {
  if (ms === null) return "";
  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(ms < 10000 ? 2 : 1)} s`;
}

/** One line of plain text, for copying rows. */
export function entryToText(e: LogEntry): string {
  const parts = [
    formatLogTime(e.time),
    e.host ?? "-",
    e.channel.toUpperCase(),
    e.op,
    e.status === "ok" ? "OK" : e.status === "error" ? "ERRORE" : "IN CORSO",
    e.detail,
  ];
  if (e.durationMs !== null) parts.push(formatDuration(e.durationMs));
  if (e.extra) parts.push(e.extra);
  if (e.error) parts.push(e.error);
  return parts.join("\t");
}
