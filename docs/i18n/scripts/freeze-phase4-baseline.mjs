#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { execFileSync } from "node:child_process";

const root = path.resolve(process.argv[2] || ".");
const outputPath = path.join(
  root,
  "docs/i18n/audit/phase-4-content/pre-edit-baseline.json",
);

const sha256 = (content) =>
  crypto.createHash("sha256").update(content).digest("hex").toUpperCase();

async function walk(relativeDirectory, predicate = () => true) {
  const absoluteDirectory = path.join(root, relativeDirectory);
  const entries = await fs.readdir(absoluteDirectory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const relativePath = path.posix.join(
      relativeDirectory.replaceAll("\\", "/"),
      entry.name,
    );
    if (entry.isDirectory()) {
      files.push(...(await walk(relativePath, predicate)));
    } else if (entry.isFile() && predicate(relativePath)) {
      files.push(relativePath);
    }
  }
  return files;
}

async function inspect(relativePath) {
  const absolutePath = path.join(root, relativePath);
  const [content, stat] = await Promise.all([
    fs.readFile(absolutePath),
    fs.stat(absolutePath),
  ]);
  return {
    path: relativePath,
    bytes: content.length,
    sha256: sha256(content),
    modifiedAt: stat.mtime.toISOString(),
  };
}

const sourceFiles = [
  ...(await walk("frontend/src-tauri/templates", (file) => file.endsWith(".json"))),
  ...(await walk(
    "frontend/src-tauri/src/summary/templates",
    (file) => file.endsWith(".rs"),
  )),
  "frontend/src-tauri/schemas/template-v2.schema.json",
  "frontend/src-tauri/src/summary/commands.rs",
  "frontend/src-tauri/src/summary/service.rs",
  "frontend/src-tauri/src/summary/metadata.rs",
  "frontend/src-tauri/src/summary/template_commands_v2.rs",
  "frontend/src-tauri/src/summary/template_snapshot.rs",
  "frontend/src-tauri/src/summary/mod.rs",
  "frontend/src-tauri/src/lib.rs",
  "frontend/src/services/templateService.ts",
  "frontend/src/types/summary-template.ts",
  "frontend/src/lib/template-library.ts",
  "frontend/src/lib/template-editor.ts",
  "frontend/src/hooks/useTemplateLibrary.ts",
  "frontend/src/hooks/useTemplateEditor.ts",
  "frontend/src/hooks/meeting-details/useTemplates.ts",
  "frontend/src/hooks/meeting-details/useSummaryGeneration.ts",
  "docs/i18n/baseline/template-content-inventory.json",
].sort();

const inventory = JSON.parse(
  await fs.readFile(
    path.join(root, "docs/i18n/baseline/template-content-inventory.json"),
    "utf8",
  ),
);
const templateFiles = sourceFiles.filter(
  (file) => file.startsWith("frontend/src-tauri/templates/") && file.endsWith(".json"),
);
const legacyTemplates = [];
for (const file of templateFiles) {
  const value = JSON.parse(await fs.readFile(path.join(root, file), "utf8"));
  legacyTemplates.push({
    file,
    name: value.name,
    sections: Array.isArray(value.sections) ? value.sections.length : null,
    hasStableId: typeof value.id === "string" && value.id.length > 0,
    hasLocale: typeof value.locale === "string" && value.locale.length > 0,
    hasVersion: Number.isInteger(value.version) && value.version > 0,
    schemaVersion: value.schema_version ?? null,
  });
}

let gitHead = null;
let relevantGitStatus = null;
try {
  gitHead = execFileSync("git", ["rev-parse", "HEAD"], {
    cwd: root,
    encoding: "utf8",
  }).trim();
  relevantGitStatus = execFileSync(
    "git",
    [
      "status",
      "--short",
      "--",
      "frontend/src-tauri/templates",
      "frontend/src-tauri/schemas",
      "frontend/src-tauri/src/summary",
      "frontend/src/services/templateService.ts",
      "frontend/src/types/summary-template.ts",
      "frontend/src/lib/template-library.ts",
      "frontend/src/lib/template-editor.ts",
    ],
    { cwd: root, encoding: "utf8" },
  )
    .trim()
    .split(/\r?\n/)
    .filter(Boolean);
} catch {
  // The source hashes remain authoritative if Git metadata is unavailable.
}

const report = {
  phase: "15.7-stage-4-template-and-ai-content-i18n",
  generatedAt: new Date().toISOString(),
  gitHead,
  relevantGitStatus,
  invariants: {
    contentCandidateCount: inventory.entries.length,
    legacyTemplateCount: legacyTemplates.length,
    formalReleaseProtected: true,
  },
  legacyTemplates,
  sourceFiles: await Promise.all(sourceFiles.map(inspect)),
  protectedReleaseBinary: await inspect("target/release/meetily.exe"),
};

await fs.mkdir(path.dirname(outputPath), { recursive: true });
await fs.writeFile(outputPath, `${JSON.stringify(report, null, 2)}\n`);
process.stdout.write(
  `${JSON.stringify({
    outputPath,
    sourceFiles: report.sourceFiles.length,
    contentCandidates: report.invariants.contentCandidateCount,
    legacyTemplates: report.invariants.legacyTemplateCount,
    protectedReleaseBinary: report.protectedReleaseBinary,
  })}\n`,
);
