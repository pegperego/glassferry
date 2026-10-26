// Drag & drop of file selections between the two panes.
//
// The source side is encoded in the MIME type because `dragover` handlers
// can read the types but not the data.

export type Side = "local" | "remote";

export interface DragPayload {
  side: Side;
  dir: string;
  names: string[];
}

const mimeFor = (side: Side) => `application/x-sfm-${side}`;

export function setDragPayload(dataTransfer: DataTransfer, payload: DragPayload): void {
  dataTransfer.setData(mimeFor(payload.side), JSON.stringify(payload));
  dataTransfer.setData("text/plain", payload.names.join("\n"));
  dataTransfer.effectAllowed = "copy";
}

/** Side of an in-app drag, or null for anything else. */
export function dragSourceSide(dataTransfer: DataTransfer): Side | null {
  if (dataTransfer.types.includes(mimeFor("local"))) return "local";
  if (dataTransfer.types.includes(mimeFor("remote"))) return "remote";
  return null;
}

export function getDragPayload(dataTransfer: DataTransfer): DragPayload | null {
  const side = dragSourceSide(dataTransfer);
  if (side === null) return null;
  try {
    return JSON.parse(dataTransfer.getData(mimeFor(side))) as DragPayload;
  } catch {
    return null;
  }
}

/** Groups absolute local paths by parent directory (Finder drops). */
export function groupByParent(paths: string[]): Map<string, string[]> {
  const groups = new Map<string, string[]>();
  for (const path of paths) {
    const trimmed = path.replace(/\/+$/, "");
    const index = trimmed.lastIndexOf("/");
    if (index < 0) continue;
    const dir = index === 0 ? "/" : trimmed.slice(0, index);
    const name = trimmed.slice(index + 1);
    groups.set(dir, [...(groups.get(dir) ?? []), name]);
  }
  return groups;
}
