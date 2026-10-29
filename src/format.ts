const UNITS = ["B", "KB", "MB", "GB", "TB"];

export function formatSize(bytes: number | null): string {
  if (bytes === null) return "";
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit++;
  }
  return unit === 0 ? `${value} ${UNITS[0]}` : `${value.toFixed(1)} ${UNITS[unit]}`;
}

const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "short", timeStyle: "short" });

export function formatTime(epochSeconds: number | null): string {
  return epochSeconds === null ? "" : dateFormat.format(new Date(epochSeconds * 1000));
}

/** `rwxr-xr-x` from the low 9 permission bits. */
export function formatPermissions(mode: number | null): string {
  if (mode === null) return "";
  const flags = "rwxrwxrwx";
  let out = "";
  for (let i = 0; i < 9; i++) {
    out += mode & (1 << (8 - i)) ? flags[i] : "-";
  }
  return out;
}

/** `owner:group`, with the numeric ids when the names are unknown. */
export function formatOwner(entry: { uid: number | null; gid: number | null; owner: string | null; group: string | null }): string {
  if (entry.uid === null && entry.gid === null) return "";
  return `${entry.owner ?? entry.uid ?? "?"}:${entry.group ?? entry.gid ?? "?"}`;
}

export function ownerTitle(entry: { uid: number | null; gid: number | null; owner: string | null; group: string | null }): string {
  if (entry.uid === null && entry.gid === null) return "";
  return `proprietario ${entry.owner ?? "?"} (uid ${entry.uid ?? "?"}), gruppo ${entry.group ?? "?"} (gid ${entry.gid ?? "?"})`;
}
