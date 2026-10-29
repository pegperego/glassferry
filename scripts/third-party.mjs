#!/usr/bin/env node
// Collects the third-party components shipped in the app, with their
// licenses and license texts, into src/generated/third-party.json for the
// "Informazioni" dialog.
//
// Rust: crates of the normal dependency graph (no dev-dependencies) for the
// targets we ship, from `cargo tree`; license field and files from
// `cargo metadata`. JavaScript: production packages from `pnpm licenses`.
// License texts are the LICENSE/COPYING/NOTICE files of each package,
// deduplicated. The output is deterministic (sorted, no timestamps).
//
// Run by `pnpm build`; run it by hand after changing dependencies.

import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, statSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const manifest = join(root, "src-tauri", "Cargo.toml");
const output = join(root, "src", "generated", "third-party.json");
const TARGETS = ["aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"];
const LICENSE_FILE = /^(licen[cs]e|copying|notice|copyright|unlicense)([-._].*)?$/i;
const MAX_TEXT = 64 * 1024;

const run = (cmd, args) => execFileSync(cmd, args, { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });

function licenseFiles(dir) {
  let names;
  try {
    names = readdirSync(dir);
  } catch {
    return [];
  }
  return names
    .filter((name) => LICENSE_FILE.test(name))
    .sort()
    .flatMap((name) => {
      const path = join(dir, name);
      try {
        if (!statSync(path).isFile()) return [];
        return [readFileSync(path, "utf8").slice(0, MAX_TEXT).replace(/\r\n/g, "\n").trim()];
      } catch {
        return [];
      }
    });
}

// ---- Rust ----

const shipped = new Set();
for (const target of TARGETS) {
  const tree = run("cargo", ["tree", "--quiet", "--manifest-path", manifest, "-e", "normal", "--target", target, "--prefix", "none", "-f", "{p}"]);
  for (const line of tree.split("\n")) {
    const match = /^(\S+) v(\S+)/.exec(line.trim());
    if (match) shipped.add(`${match[1]}@${match[2]}`);
  }
}
const metadata = JSON.parse(run("cargo", ["metadata", "--quiet", "--manifest-path", manifest, "--format-version", "1"]));
const ownName = metadata.packages.find((p) => p.manifest_path === manifest)?.name;

const packages = [];
for (const pkg of metadata.packages) {
  if (pkg.name === ownName || !shipped.has(`${pkg.name}@${pkg.version}`)) continue;
  packages.push({
    name: pkg.name,
    version: pkg.version,
    license: pkg.license ?? (pkg.license_file ? "vedi testo" : "non indicata"),
    source: "crates.io",
    url: pkg.repository ?? `https://crates.io/crates/${pkg.name}`,
    texts: licenseFiles(dirname(pkg.manifest_path)),
  });
}

// ---- JavaScript ----

const npm = JSON.parse(run("pnpm", ["licenses", "list", "--prod", "--json"]));
for (const entries of Object.values(npm)) {
  for (const pkg of entries) {
    for (const [i, version] of pkg.versions.entries()) {
      packages.push({
        name: pkg.name,
        version,
        license: pkg.license,
        source: "npm",
        url: pkg.homepage ?? `https://www.npmjs.com/package/${pkg.name}`,
        texts: licenseFiles(pkg.paths[i] ?? pkg.paths[0]),
      });
    }
  }
}

// ---- output: texts deduplicated, referenced by index ----

packages.sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version) || a.source.localeCompare(b.source));
const texts = [];
const index = new Map();
const result = {
  packages: packages.map((pkg) => ({
    ...pkg,
    texts: pkg.texts.map((text) => {
      if (!index.has(text)) {
        index.set(text, texts.length);
        texts.push(text);
      }
      return index.get(text);
    }),
  })),
  texts,
};
mkdirSync(dirname(output), { recursive: true });
writeFileSync(output, JSON.stringify(result) + "\n");
const missing = result.packages.filter((p) => p.texts.length === 0).length;
console.log(`third-party: ${result.packages.length} componenti, ${texts.length} testi di licenza${missing ? `, ${missing} senza file di licenza` : ""}`);
