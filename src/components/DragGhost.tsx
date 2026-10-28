import { useDragState } from "../pointerDrag";

/** Label following the pointer during a pane-to-pane drag. */
export function DragGhost() {
  const drag = useDragState();
  if (!drag) return null;
  const { payload, target } = drag;
  const what = payload.names.length === 1 ? payload.names[0] : `${payload.names.length} elementi`;
  const action = target === null ? "non qui" : target.side === payload.side ? "sposta" : "copia";
  return (
    <div className={target ? "drag-ghost" : "drag-ghost refused"} style={{ left: drag.x + 14, top: drag.y + 10 }}>
      <span>{what}</span>
      <span className="drag-action">{action}</span>
    </div>
  );
}
