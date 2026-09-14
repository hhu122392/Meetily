#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { execFileSync } from "node:child_process";

const root = path.resolve(process.argv[2] || ".");
const manifest = JSON.parse(await fs.readFile(path.join(root, "docs/i18n/phase-2/phase2-batches.json"), "utf8"));
const batch = manifest.batches["2F"];
const resourceFiles = batch.namespaces.flatMap((namespace) => [
  `frontend/src/i18n/locales/en/${namespace}.json`,
  `frontend/src/i18n/locales/zh-CN/${namespace}.json`,
  `docs/i18n/baseline/locales/en/${namespace}.json`,
]);
const files = [...new Set([...batch.sources, ...resourceFiles])];

const sha256 = (buffer) => crypto.createHash("sha256").update(buffer).digest("hex").toUpperCase();
const git = (...args) => execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
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
      const headContent = execFileSync("git", ["show", `HEAD:${normalizedPath}`], { cwd: root });
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

const output = {
  phase: "15.5-stage-2F-updates-analytics-about-beta",
  frozenAt: new Date().toISOString(),
  gitHead: git("rev-parse", "HEAD"),
  gitBranch: git("branch", "--show-current"),
  ownership: {
    included: batch.sources,
    namespaces: batch.namespaces,
    readOnlyIntegration: [
      "frontend/src/app/layout.tsx",
      "frontend/src/app/settings/page.tsx",
      "frontend/src/components/PreferenceSettings.tsx",
      "frontend/src/components/SettingTabs.tsx",
      "frontend/src/components/Sidebar/index.tsx",
      "frontend/src/app/settings/templates/**",
      "frontend/src/components/templates/**",
      "frontend/src/components/SummaryTemplateSettings.tsx",
    ],
  },
  entries,
};

const outputPath = path.join(root, "docs/i18n/audit/phase-2-react/2F/pre-edit-baseline.json");
await fs.mkdir(path.dirname(outputPath), { recursive: true });
await fs.writeFile(outputPath, JSON.stringify(output, null, 2) + "\n");
process.stdout.write(JSON.stringify({ files: entries.length, differsFromHead: entries.filter((entry) => entry.differsFromHead).length, outputPath }) + "\n");
