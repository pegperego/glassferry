// POSIX path helpers for remote paths (never use them for local paths).

export function joinPath(dir: string, name: string): string {
  return dir.endsWith("/") ? dir + name : `${dir}/${name}`;
}

export function parentPath(path: string): string {
  if (path === "/") return "/";
  const trimmed = path.replace(/\/+$/, "");
  const index = trimmed.lastIndexOf("/");
  return index <= 0 ? "/" : trimmed.slice(0, index);
}

/** Breadcrumb segments: [{ name: "/", path: "/" }, { name: "home", path: "/home" }, ...] */
export function pathSegments(path: string): { name: string; path: string }[] {
  const segments = [{ name: "/", path: "/" }];
  let current = "";
  for (const part of path.split("/").filter(Boolean)) {
    current += `/${part}`;
    segments.push({ name: part, path: current });
  }
  return segments;
}
