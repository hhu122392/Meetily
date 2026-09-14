#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { execFileSync } from "node:child_process";

const root = path.resolve(process.argv[2] || ".");
const files = [
  "frontend/src-tauri/src/lib.rs",
  "frontend/src-tauri/src/tray.rs",
  "frontend/src-tauri/src/notifications/mod.rs",
  "frontend/src-tauri/src/notifications/types.rs",
  "frontend/src-tauri/src/notifications/commands.rs",
  "frontend/src-tauri/src/notifications/manager.rs",
  "frontend/src-tauri/src/notifications/settings.rs",
  "frontend/src-tauri/src/notifications/system.rs",
  "frontend/src-tauri/src/audio/mod.rs",
  "frontend/src-tauri/src/lib_old_complex.rs",
  "frontend/src-tauri/src/audio/core-old.rs",
  "frontend/src-tauri/src/audio/recording_saver_old.rs",
  "frontend/src/i18n/I18nProvider.tsx",
  "frontend/src/i18n/locale.ts",
  "frontend/src/i18n/types.ts",
  "target/release/docs/en.catalog.json",
];

const sha256 = (buffer) =>
  crypto.createHash("sha256").update(buffer).digest("hex").toUpperCase();
const git = (...args) =>
  execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
const tracked = new Set(git("ls-files").split(/\r?\n/).filter(Boolean));
const entries = [];

for (const relativePath of files) {
  const normalizedPath = relativePath.replaceAll("\\", "/");
  const absolutePath = path.join(root, relativePath);
  const content = await fs.readFile(absolutePath);
  let headSha256 = null;
  let headAvailable = false;
  if (tracked.has(normalizedPath)) {
    try {
      const headContent = execFileSync("git", ["show", `HEAD:${normalizedPath}`], {
        cwd: root,
      });
      headSha256 = sha256(headContent);
      headAvailable = true;
    } catch {}
  }
  entries.push({
    path: normalizedPath,
    bytes: content.length,
    sha256: sha256(content),
    trackedAtHead: tracked.has(normalizedPath),
    headAvailable,
    headSha256,
    differsFromHead: headAvailable ? headSha256 !== sha256(content) : null,
  });
}

const oldSourceModuleReferences = [];
for (const relativePath of [
  "frontend/src-tauri/src/lib.rs",
  "frontend/src-tauri/src/audio/mod.rs",
]) {
  const text = await fs.readFile(path.join(root, relativePath), "utf8");
  for (const token of ["lib_old_complex", "core-old", "recording_saver_old"]) {
    if (text.includes(token)) oldSourceModuleReferences.push({ relativePath, token });
  }
}

const output = {
  phase: "15.6-stage-3-tauri-native-i18n",
  frozenAt: new Date().toISOString(),
  gitHead: git("rev-parse", "HEAD"),
  gitBranch: git("branch", "--show-current"),
  protectedReleaseBinary: await (async () => {
    const relativePath = "target/release/meetily.exe";
    const content = await fs.readFile(path.join(root, relativePath));
    return { path: relativePath, bytes: content.length, sha256: sha256(content) };
  })(),
  isolatedBuildTarget: "target-phase3-native",
  ownership: {
    implementation: [
      "frontend/src-tauri/src/i18n/**",
      "frontend/src-tauri/src/tray.rs",
      "frontend/src-tauri/src/notifications/**",
      "frontend/src/i18n/I18nProvider.tsx",
      "frontend/src/lib/native-i18n.ts",
    ],
    integrationOnly: ["frontend/src-tauri/src/lib.rs"],
    readOnlyOldSource: [
      "frontend/src-tauri/src/lib_old_complex.rs",
      "frontend/src-tauri/src/audio/core-old.rs",
      "frontend/src-tauri/src/audio/recording_saver_old.rs",
    ],
  },
  oldSourceModuleReferences,
  oldSourceExcludedFromModuleGraph: oldSourceModuleReferences.length === 0,
  entries,
};

const outputPath = path.join(
  root,
  "docs/i18n/audit/phase-3-native/pre-edit-baseline.json",
);
await fs.mkdir(path.dirname(outputPath), { recursive: true });
await fs.writeFile(outputPath, JSON.stringify(output, null, 2) + "\n");
process.stdout.write(
  JSON.stringify({
    files: entries.length,
    differsFromHead: entries.filter((entry) => entry.differsFromHead).length,
    oldSourceExcludedFromModuleGraph: output.oldSourceExcludedFromModuleGraph,
    outputPath,
  }) + "\n",
);
