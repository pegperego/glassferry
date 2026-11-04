// Path helpers for the panes: POSIX paths (remote, and local on macOS/Linux).
// Windows local paths come from the backend with `/` ("C:/Users/x"); with
// `drives` set (local pane on Windows), "/" is a virtual root listing the
// drives and "C:/" is a drive's root. Remote panes never set it: a remote
// folder may well be called "C:".

/** The app runs on Windows (local paths with drives). */
export const IS_WINDOWS = typeof navigator !== "undefined" && navigator.userAgent.includes("Windows");

const DRIVE = /^[A-Za-z]:$/;
const DRIVE_ROOT = /^[A-Za-z]:\/?$/;

export function joinPath(dir: string, name: string, drives = false): string {
  if (drives && dir === "/" && DRIVE.test(name)) return `${name}/`;
  return dir.endsWith("/") ? dir + name : `${dir}/${name}`;
}

export function parentPath(path: string, drives = false): string {
  if (path === "/") return "/";
  if (drives && DRIVE_ROOT.test(path)) return "/";
  const trimmed = path.replace(/\/+$/, "");
  const index = trimmed.lastIndexOf("/");
  if (drives && index > 0 && DRIVE.test(trimmed.slice(0, index))) return `${trimmed.slice(0, index)}/`;
  return index <= 0 ? "/" : trimmed.slice(0, index);
}

/** Breadcrumb segments: [{ name: "/", path: "/" }, { name: "home", path: "/home" }, ...];
 * with `drives`: [{ name: "/", path: "/" }, { name: "C:", path: "C:/" }, { name: "Users", path: "C:/Users" }, ...] */
export function pathSegments(path: string, drives = false): { name: string; path: string }[] {
  const segments = [{ name: "/", path: "/" }];
  const parts = path.split("/").filter(Boolean);
  let current = "";
  parts.forEach((part, i) => {
    if (drives && i === 0 && DRIVE.test(part)) {
      current = part;
      segments.push({ name: part, path: `${part}/` });
      return;
    }
    current += `/${part}`;
    segments.push({ name: part, path: current });
  });
  return segments;
}

/** An absolute path for this pane: "/…", or "C:/…" with `drives`. */
export function isAbsolutePath(path: string, drives = false): boolean {
  return path.startsWith("/") || (drives && /^[A-Za-z]:\//.test(path));
}

/** Paths from the system (e.g. files dropped from Explorer) in the panes' form. */
export function toPanePath(path: string): string {
  return IS_WINDOWS ? path.replace(/\\/g, "/") : path;
}
