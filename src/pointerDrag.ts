// In-app drag & drop between panes, built on pointer events.
//
// HTML5 drag events never reach the page in the native macOS window: Tauri's
// drag-drop handler (needed to receive files dropped from the Finder with
// their paths) claims every drag over the webview. Pointer events are not
// affected, so pane-to-pane drags track the pointer themselves: each pane
// registers a drop zone, the zone under the pointer is found with
// `elementFromPoint` and the `data-pane` / `data-entry` attributes.

import { useSyncExternalStore, type PointerEvent as ReactPointerEvent } from "react";
import type { DragPayload, Side } from "./dnd";

/** Pixels the pointer must move before a press becomes a drag. */
const THRESHOLD = 4;

export interface DropZone {
  /** `entry`: folder row under the pointer, null for the pane itself. */
  accepts: (payload: DragPayload, entry: string | null) => boolean;
  drop: (payload: DragPayload, entry: string | null) => void;
}

export interface DragTarget {
  side: Side;
  entry: string | null;
}

export interface DragState {
  payload: DragPayload;
  x: number;
  y: number;
  target: DragTarget | null;
}

const zones = new Map<Side, DropZone>();
let state: DragState | null = null;
const listeners = new Set<() => void>();

function set(next: DragState | null) {
  state = next;
  document.body.classList.toggle("dragging", next !== null);
  document.body.classList.toggle("drag-refused", next !== null && next.target === null);
  for (const fn of listeners) fn();
}

export function registerDropZone(side: Side, zone: DropZone): () => void {
  zones.set(side, zone);
  return () => {
    if (zones.get(side) === zone) zones.delete(side);
  };
}

const subscribe = (fn: () => void) => {
  listeners.add(fn);
  return () => listeners.delete(fn);
};

/** The drag in progress, or null. */
export function useDragState(): DragState | null {
  return useSyncExternalStore(subscribe, () => state);
}

/** Pane and folder under the pointer that accept `payload`. */
export function hitTest(x: number, y: number, payload: DragPayload): DragTarget | null {
  const element = document.elementFromPoint(x, y);
  const paneElement = element?.closest("[data-pane]");
  if (!element || !paneElement) return null;
  const side = paneElement.getAttribute("data-pane") as Side;
  const zone = zones.get(side);
  if (!zone) return null;
  const row = element.closest("[data-entry]");
  const folder = row?.getAttribute("data-dir") === "1" ? row.getAttribute("data-entry") : null;
  if (folder !== null && zone.accepts(payload, folder)) return { side, entry: folder };
  if (zone.accepts(payload, null)) return { side, entry: null };
  return null;
}

/**
 * Call from `onPointerDown` of a draggable row. `payload` is computed when
 * the pointer has moved enough (a plain click stays a click).
 */
export function beginPointerDrag(event: ReactPointerEvent, payload: () => DragPayload | null) {
  if (event.button !== 0 || event.ctrlKey) return;
  const startX = event.clientX;
  const startY = event.clientY;
  let dragging: DragPayload | null = null;

  const move = (e: PointerEvent) => {
    if (!dragging) {
      if (Math.hypot(e.clientX - startX, e.clientY - startY) < THRESHOLD) return;
      dragging = payload();
      if (!dragging) return finish();
      window.getSelection()?.removeAllRanges();
    }
    set({ payload: dragging, x: e.clientX, y: e.clientY, target: hitTest(e.clientX, e.clientY, dragging) });
  };
  const up = (e: PointerEvent) => {
    const current = dragging;
    finish();
    if (!current) return;
    // The click that follows the release must not change the selection.
    window.addEventListener("click", swallow, { capture: true, once: true });
    setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
    const target = hitTest(e.clientX, e.clientY, current);
    if (target) zones.get(target.side)?.drop(current, target.entry);
  };
  const key = (e: KeyboardEvent) => {
    if (e.key === "Escape") finish();
  };
  const finish = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    window.removeEventListener("keydown", key, { capture: true });
    if (state) set(null);
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
  window.addEventListener("keydown", key, { capture: true });
}

function swallow(e: Event) {
  e.stopPropagation();
  e.preventDefault();
}
