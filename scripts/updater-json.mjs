#!/usr/bin/env node
// Writes latest.json, the update manifest the installed apps read (see
// src-tauri/src/update.rs), from the signatures of a release's assets.
//
// Run by .github/workflows/release.yml once every build has uploaded its
// packages: tauri-action can write it too, but each matrix job rewrites it
// and they race (only one platform survives).
//
//   node scripts/updater-json.mjs <version> <dir with the .sig files> <notes file> > latest.json
//
// Keys follow tauri-plugin-updater: `{os}-{arch}` (the default package) and
// `{os}-{arch}-{bundle}` (the package the running app was installed from).

import { readFileSync, existsSync } from "node:fs";
import { join } from "node:path";

const REPOSITORY = "https://github.com/pegperego/glassferry";

/** Asset names produced by tauri build for `version`, per updater key. */
function assets(version) {
  const v = version;
  return {
    "darwin-aarch64": `Glassferry_aarch64.app.tar.gz`,
    "darwin-aarch64-app": `Glassferry_aarch64.app.tar.gz`,
    "darwin-x86_64": `Glassferry_x64.app.tar.gz`,
    "darwin-x86_64-app": `Glassferry_x64.app.tar.gz`,
    "linux-x86_64": `Glassferry_${v}_amd64.AppImage`,
    "linux-x86_64-appimage": `Glassferry_${v}_amd64.AppImage`,
    "linux-x86_64-deb": `Glassferry_${v}_amd64.deb`,
    "linux-x86_64-rpm": `Glassferry-${v}-1.x86_64.rpm`,
    "linux-aarch64": `Glassferry_${v}_aarch64.AppImage`,
    "linux-aarch64-appimage": `Glassferry_${v}_aarch64.AppImage`,
    "linux-aarch64-deb": `Glassferry_${v}_arm64.deb`,
    "linux-aarch64-rpm": `Glassferry-${v}-1.aarch64.rpm`,
    "windows-x86_64": `Glassferry_${v}_x64-setup.exe`,
    "windows-x86_64-nsis": `Glassferry_${v}_x64-setup.exe`,
    "windows-x86_64-msi": `Glassferry_${v}_x64_en-US.msi`,
  };
}

function main([version, dir, notesFile]) {
  if (!version || !dir || !notesFile) {
    console.error("uso: updater-json.mjs <versione> <cartella .sig> <file note>");
    process.exit(2);
  }
  const platforms = {};
  const missing = [];
  for (const [key, name] of Object.entries(assets(version))) {
    const sig = join(dir, `${name}.sig`);
    if (!existsSync(sig)) {
      missing.push(`${name}.sig`);
      continue;
    }
    platforms[key] = {
      signature: readFileSync(sig, "utf8").trim(),
      url: `${REPOSITORY}/releases/download/v${version}/${encodeURIComponent(name)}`,
    };
  }
  if (missing.length > 0) {
    console.error(`firme mancanti: ${[...new Set(missing)].join(", ")}`);
    process.exit(1);
  }
  const manifest = {
    version,
    notes: readFileSync(notesFile, "utf8").trim(),
    pub_date: new Date().toISOString(),
    platforms,
  };
  process.stdout.write(`${JSON.stringify(manifest, null, 2)}\n`);
}

main(process.argv.slice(2));
