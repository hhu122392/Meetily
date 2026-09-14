#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "../../..");
const outputRoot = path.join(repoRoot, "docs/i18n/audit/phase-5a1-source");
const releaseSourceBaseline =
  process.env.MEETILY_RELEASE_SOURCE_BASELINE ?? "0281737d87d26352fb0adc78c8c0975f691b23d1";
const releaseSourceRef = process.env.MEETILY_RELEASE_SOURCE_REF || null;
const generatedPaths = new Set([
  "docs/i18n/audit/phase-5a1-source/source-inventory.json",
  "docs/i18n/audit/phase-5a1-source/sensitive-content-audit.json",
]);

function git(args) {
  return execFileSync("git", args, {
    cwd: repoRoot,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  }).trimEnd();
}

function gitBuffer(args) {
  return execFileSync("git", args, {
    cwd: repoRoot,
    maxBuffer: 64 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function lines(value) {
  return value ? value.split(/\r?\n/u).filter(Boolean) : [];
}

function sha256Buffer(buffer) {
  return crypto.createHash("sha256").update(buffer).digest("hex").toUpperCase();
}

function sha256(filePath) {
  return sha256Buffer(fs.readFileSync(filePath));
}

function normalize(relativePath) {
  return relativePath.replaceAll("\\", "/");
}

function classify(relativePath, tracked, hasContentDiff) {
  if (relativePath === ".gitignore") {
    return {
      category: "repository-hygiene",
      disposition: "INCLUDE_REPOSITORY_HYGIENE",
      commitGroup: "01-repository-hygiene",
      reason: "Prevents isolated build outputs and recovery caches from entering the release-source view.",
    };
  }

  if (relativePath === "frontend/next.config.js" && tracked && !hasContentDiff) {
    return {
      category: "metadata-only",
      disposition: "EXCLUDE_STAT_ONLY",
      commitGroup: null,
      reason: "Git reports a worktree status change but no content diff is present.",
    };
  }

  if (relativePath === "frontend/src-tauri/.cargo/config.toml") {
    return {
      category: "metadata-only",
      disposition: "EXCLUDE_LINE_ENDING_ONLY",
      commitGroup: null,
      reason: "The only diff is the final newline; it is unrelated to localization or release behavior.",
    };
  }

  if (relativePath === "frontend/package.json") {
    return {
      category: "build-and-dependencies",
      disposition: "REVIEW_MIXED_CHANGE",
      commitGroup: "02-build-and-dependencies",
      reason: "Required i18n/test dependencies and scripts share a hunk with an unrelated dev --turbo change.",
    };
  }

  if (relativePath === "frontend/src-tauri/tauri.conf.json") {
    return {
      category: "security-sensitive-release-config",
      disposition: "REVIEW_SECURITY_SENSITIVE_BUILD",
      commitGroup: "02-build-and-dependencies",
      reason: "Contains required localized resources and installer settings plus CSP, permission, downgrade, and upgrade-code changes.",
    };
  }

  if (
    relativePath === "frontend/src/contexts/OnboardingContext.tsx" ||
    relativePath === "frontend/src/hooks/usePlatform.ts" ||
    relativePath === "frontend/src/lib/navigation-guard.ts"
  ) {
    return {
      category: "supporting-regression-fix",
      disposition: "REVIEW_SUPPORTING_CHANGE",
      commitGroup: "03-supporting-regression-fixes",
      reason: "Behavioral fix required by the tested runtime flow but not itself a translation replacement.",
    };
  }

  if (relativePath.startsWith("frontend/src-tauri/tauri.phase") && relativePath.endsWith(".conf.json")) {
    return {
      category: "audit-only-config",
      disposition: "INCLUDE_AUDIT_SUPPORT_ONLY",
      commitGroup: "07-audit-tooling",
      reason: "Isolated audit identity/configuration; never use as the production Tauri configuration.",
    };
  }

  if (relativePath.startsWith("docs/i18n/audit/phase-5a1-source/")) {
    return {
      category: "release-source-audit",
      disposition: "INCLUDE_RELEASE_SOURCE_AUDIT",
      commitGroup: "08-documentation",
      reason: "Compact 5A-1 source-boundary evidence must remain versioned so the release-source plan is independently verifiable after cloning.",
    };
  }

  if (relativePath.startsWith("docs/i18n/audit/")) {
    return {
      category: "generated-audit-evidence",
      disposition: "EVIDENCE_ARCHIVE_EXCLUDE_SOURCE",
      commitGroup: null,
      reason: "Generated JSON/PNG evidence belongs in the immutable audit archive, not the production source commit.",
    };
  }

  if (relativePath.startsWith("docs/i18n/scripts/")) {
    return {
      category: "audit-tooling",
      disposition: "INCLUDE_AUDIT_TOOLING_SEPARATE_COMMIT",
      commitGroup: "07-audit-tooling",
      reason: "Reproducible audit tooling is useful but is not runtime product code.",
    };
  }

  if (relativePath.startsWith("docs/i18n/")) {
    return {
      category: "architecture-and-review-docs",
      disposition: "INCLUDE_DOCUMENTATION_SEPARATE_COMMIT",
      commitGroup: "08-documentation",
      reason: "Architecture, glossary, baseline, review, and release-plan material is versionable separately from runtime code.",
    };
  }

  if (relativePath.startsWith("frontend/tests/")) {
    return {
      category: "automated-tests",
      disposition: "INCLUDE_TESTS",
      commitGroup: "06-tests",
      reason: "Regression coverage for localization, templates, navigation, and state isolation.",
    };
  }

  if (relativePath.startsWith("frontend/src/i18n/") || relativePath === "frontend/src/lib/native-i18n.ts") {
    return {
      category: "frontend-i18n-core",
      disposition: "INCLUDE_PRODUCTION_SOURCE",
      commitGroup: "04-frontend-i18n",
      reason: "Production frontend locale resources, provider, locale normalization, and native bridge.",
    };
  }

  if (relativePath.startsWith("frontend/src-tauri/src/i18n/")) {
    return {
      category: "native-i18n-core",
      disposition: "INCLUDE_PRODUCTION_SOURCE",
      commitGroup: "05-native-and-template-engine",
      reason: "Production Rust/Tauri locale resources and native error localization.",
    };
  }

  if (relativePath.startsWith("frontend/src-tauri/templates/")) {
    return {
      category: "localized-built-in-templates",
      disposition: "INCLUDE_PRODUCTION_SOURCE",
      commitGroup: "05-native-and-template-engine",
      reason: "Localized built-in meeting templates bundled with the application.",
    };
  }

  if (
    relativePath.startsWith("frontend/src-tauri/src/") ||
    relativePath.startsWith("frontend/src-tauri/schemas/")
  ) {
    return {
      category: "native-product",
      disposition: "INCLUDE_PRODUCTION_SOURCE",
      commitGroup: "05-native-and-template-engine",
      reason: "Rust/Tauri product implementation for localized native surfaces and template management.",
    };
  }

  if (relativePath.startsWith("frontend/src/")) {
    return {
      category: "frontend-product",
      disposition: "INCLUDE_PRODUCTION_SOURCE",
      commitGroup: "04-frontend-i18n",
      reason: "Frontend localization migration or localized meeting-template product UI.",
    };
  }

  if (relativePath === "frontend/src-tauri/scripts/nsis-installer-hooks.nsh") {
    return {
      category: "installer-source",
      disposition: "INCLUDE_PRODUCTION_BUILD",
      commitGroup: "02-build-and-dependencies",
      reason: "Production NSIS localization/upgrade hook referenced by tauri.conf.json.",
    };
  }

  if (
    relativePath === "Cargo.lock" ||
    relativePath === "frontend/package.json" ||
    relativePath === "frontend/pnpm-lock.yaml" ||
    relativePath === "frontend/pnpm-workspace.yaml" ||
    relativePath === "frontend/src-tauri/Cargo.toml" ||
    relativePath === "frontend/src-tauri/build.rs"
  ) {
    return {
      category: "build-and-dependencies",
      disposition: "INCLUDE_PRODUCTION_BUILD",
      commitGroup: "02-build-and-dependencies",
      reason: "Required dependency locks, manifests, workspace policy, or cross-platform build support.",
    };
  }

  return {
    category: "unclassified",
    disposition: "BLOCK_UNCLASSIFIED",
    commitGroup: null,
    reason: "No approved release-source classification rule matched this path.",
  };
}

const gitHead = git(["rev-parse", "HEAD"]);
const gitBranch = git(["branch", "--show-current"]);
const statusLines = lines(git(["status", "--porcelain=v1", "--untracked-files=normal"]));
const trackedStatusPaths = statusLines
  .filter((line) => !line.startsWith("??"))
  .map((line) => normalize(line.slice(3)));
const sourceDiffArguments = releaseSourceRef
  ? [releaseSourceBaseline, releaseSourceRef, "--"]
  : [releaseSourceBaseline, "--"];
const diffPaths = lines(
  git(["diff", "--name-only", "--no-ext-diff", ...sourceDiffArguments]),
).map(normalize);
const untrackedPaths = lines(git(["ls-files", "--others", "--exclude-standard"])).map(normalize);
const pinnedMetadataPaths = trackedStatusPaths.filter(
  (relativePath) =>
    relativePath === "frontend/next.config.js" ||
    relativePath === "frontend/src-tauri/.cargo/config.toml",
);
const statusPathsForInventory = releaseSourceRef ? pinnedMetadataPaths : trackedStatusPaths;
const allPaths = [...new Set([...diffPaths, ...statusPathsForInventory, ...untrackedPaths])]
  .filter((relativePath) => !generatedPaths.has(relativePath))
  .sort((left, right) => left.localeCompare(right));
const diffPathSet = new Set(diffPaths);
const untrackedSet = new Set(untrackedPaths);

const numstat = new Map();
for (const line of lines(
  git(["diff", "--numstat", "--no-ext-diff", ...sourceDiffArguments]),
)) {
  const [added, deleted, relativePath] = line.split("\t");
  if (relativePath) numstat.set(normalize(relativePath), { added, deleted });
}

const entryBuffers = new Map();
const entries = allPaths.map((relativePath) => {
  const absolutePath = path.join(repoRoot, relativePath);
  const tracked = !untrackedSet.has(relativePath);
  const classification = classify(relativePath, tracked, diffPathSet.has(relativePath));
  const useReleaseSourceBlob = Boolean(releaseSourceRef && tracked && classification.commitGroup);
  const content = useReleaseSourceBlob
    ? gitBuffer(["show", `${releaseSourceRef}:${relativePath}`])
    : fs.readFileSync(absolutePath);
  entryBuffers.set(relativePath, content);
  return {
    path: relativePath,
    state: tracked ? "MODIFIED_TRACKED" : "UNTRACKED",
    bytes: content.length,
    sha256: sha256Buffer(content),
    contentSource: useReleaseSourceBlob ? `git:${releaseSourceRef}` : "worktree",
    lineStats: numstat.get(relativePath) ?? null,
    ...classification,
  };
});

function aggregate(field) {
  const result = new Map();
  for (const entry of entries) result.set(entry[field], (result.get(entry[field]) ?? 0) + 1);
  return Object.fromEntries([...result.entries()].sort(([left], [right]) => left.localeCompare(right)));
}

const ignoreSamples = [
  "target-phase5-release/release/meetily.exe",
  "target-phase2-p2-200-msi-vm-kit-20260823-r2.zip",
  "target-tools/node/node.exe",
  "frontend/.next.corrupt-20260823-0648/package.json",
].map((relativePath) => {
  let rule = null;
  try {
    rule = git(["check-ignore", "-v", "--", relativePath]);
  } catch {
    // A non-zero status means the file is not ignored.
  }
  return { path: relativePath, ignored: Boolean(rule), rule };
});

const formalArtifacts = [
  "target/release/meetily.exe",
  "target/release/bundle/nsis/meetily_0.4.0_x64-setup.exe",
].map((relativePath) => {
  const absolutePath = path.join(repoRoot, relativePath);
  return {
    path: relativePath,
    exists: fs.existsSync(absolutePath),
    bytes: fs.existsSync(absolutePath) ? fs.statSync(absolutePath).size : null,
    sha256: fs.existsSync(absolutePath) ? sha256(absolutePath) : null,
  };
});

const inventory = {
  schemaVersion: 1,
  phase: "5A-1-release-source-consolidation",
  generatedAt: new Date().toISOString(),
  git: {
    releaseSourceBaseline,
    releaseSourceRef,
    head: gitHead,
    branch: gitBranch,
    stagedEntries: lines(git(["diff", "--cached", "--name-only"])).length,
    statusEntriesAfterIgnoreHardening: statusLines.length,
    trackedStatusEntries: trackedStatusPaths.length,
    baselineDiffEntries: diffPaths.length,
    untrackedFileEntries: untrackedPaths.length,
  },
  startupBaselineBeforeIgnoreHardening: {
    statusEntries: 171,
    trackedStatusEntries: 108,
    normalUntrackedEntries: 63,
    expandedUntrackedFiles: 216215,
    isolatedBuildRecoveryAndArchiveFiles: 215373,
  },
  ignoreHardening: {
    expandedUntrackedFilesAfter: untrackedPaths.length,
    removedFromReleaseSourceView: 215373,
    samples: ignoreSamples,
  },
  formalArtifacts,
  summary: {
    inventoriedFiles: entries.length,
    byState: aggregate("state"),
    byCategory: aggregate("category"),
    byDisposition: aggregate("disposition"),
    unclassified: entries.filter((entry) => entry.disposition === "BLOCK_UNCLASSIFIED").length,
    reviewRequired: entries.filter((entry) => entry.disposition.startsWith("REVIEW_")).length,
  },
  entries,
};

const textExtensions = new Set([
  ".css", ".d.ts", ".html", ".js", ".json", ".lock", ".md", ".mjs", ".nsh",
  ".ps1", ".rs", ".toml", ".ts", ".tsx", ".txt", ".yaml", ".yml",
]);
const strongSecretPatterns = [
  ["private-key", /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/gu],
  ["openai-style-key", /\bsk-[A-Za-z0-9_-]{20,}\b/gu],
  ["github-token", /\bgh[pousr]_[A-Za-z0-9]{20,}\b/gu],
  ["aws-access-key", /\bAKIA[0-9A-Z]{16}\b/gu],
  ["slack-token", /\bxox[baprs]-[A-Za-z0-9-]{10,}\b/gu],
];
const assignmentPattern = /(?:api[_-]?key|access[_-]?token|secret|password|authorization)["']?\s*[:=]\s*["']([^"'\r\n]{8,})["']/giu;
const localPathPattern = /(?:[A-Za-z]:\\(?:Users\\[^\\\s"']+|桌面)|\/Users\/[^/\s"']+|\/home\/[^/\s"']+)/gu;
const placeholderPattern = /(?:process\.env|\$\{|\{\{|<[^>]+>|redacted|example|your[_ -]|not configured|undefined|null|open|test fixture)/iu;
const strongSecretFindings = [];
const potentialCredentialAssignments = [];
const localPathExposure = [];
let scannedTextFiles = 0;
let scannedTextBytes = 0;

for (const entry of entries) {
  const extension = path.extname(entry.path).toLowerCase();
  if (!textExtensions.has(extension) || entry.bytes > 10 * 1024 * 1024) continue;
  const content = entryBuffers.get(entry.path).toString("utf8");
  scannedTextFiles += 1;
  scannedTextBytes += entry.bytes;

  for (const [kind, pattern] of strongSecretPatterns) {
    pattern.lastIndex = 0;
    for (const match of content.matchAll(pattern)) {
      const line = content.slice(0, match.index).split(/\r?\n/u).length;
      strongSecretFindings.push({ path: entry.path, line, kind, redactedLength: match[0].length });
    }
  }

  assignmentPattern.lastIndex = 0;
  for (const match of content.matchAll(assignmentPattern)) {
    if (placeholderPattern.test(match[1])) continue;
    const line = content.slice(0, match.index).split(/\r?\n/u).length;
    potentialCredentialAssignments.push({
      path: entry.path,
      line,
      kind: "credential-like-string-assignment",
      redactedLength: match[1].length,
      disposition:
        entry.path.includes("/locales/")
          ? "TRANSLATION_COPY_FALSE_POSITIVE"
          : entry.path.startsWith("frontend/tests/") || entry.path.startsWith("docs/i18n/scripts/")
            ? "TEST_OR_AUDIT_FIXTURE"
            : "MANUAL_REVIEW_REQUIRED",
    });
  }

  localPathPattern.lastIndex = 0;
  const localMatches = [...content.matchAll(localPathPattern)];
  if (localMatches.length > 0) {
    localPathExposure.push({
      path: entry.path,
      occurrences: localMatches.length,
      disposition:
        entry.disposition === "EVIDENCE_ARCHIVE_EXCLUDE_SOURCE"
          ? "ARCHIVE_ONLY_NOT_SOURCE_COMMIT"
          : entry.path.startsWith("frontend/tests/") || entry.path.endsWith("/repository.rs")
            ? "SYNTHETIC_CROSS_PLATFORM_PATH_FIXTURE"
            : "SANITIZE_BEFORE_PUBLIC_COMMIT",
    });
  }
}

const sensitiveAudit = {
  schemaVersion: 1,
  phase: "5A-1-release-source-consolidation",
  generatedAt: new Date().toISOString(),
  scope: releaseSourceRef
    ? `Included source read from Git ref ${releaseSourceRef}; excluded metadata and archive evidence read from the worktree; binary files excluded.`
    : "Modified and untracked release-source candidates after ignore hardening; binary files excluded.",
  scannedTextFiles,
  scannedTextBytes,
  strongSecretFindingCount: strongSecretFindings.length,
  strongSecretFindings,
  potentialCredentialAssignmentCount: potentialCredentialAssignments.length,
  credentialAssignmentsRequiringManualReview: potentialCredentialAssignments.filter(
    (entry) => entry.disposition === "MANUAL_REVIEW_REQUIRED",
  ).length,
  potentialCredentialAssignments,
  localAbsolutePathFileCount: localPathExposure.length,
  localAbsolutePathOccurrences: localPathExposure.reduce((sum, entry) => sum + entry.occurrences, 0),
  localPathFilesRequiringSanitization: localPathExposure.filter(
    (entry) => entry.disposition === "SANITIZE_BEFORE_PUBLIC_COMMIT",
  ).length,
  localPathExposure,
  policy: {
    blockCommitWhenStrongSecretFound: true,
    manuallyReviewCredentialAssignments: true,
    excludeOrSanitizeLocalPathEvidenceBeforePublicCommit: true,
  },
};

fs.mkdirSync(outputRoot, { recursive: true });
fs.writeFileSync(
  path.join(outputRoot, "source-inventory.json"),
  `${JSON.stringify(inventory, null, 2)}\n`,
);
fs.writeFileSync(
  path.join(outputRoot, "sensitive-content-audit.json"),
  `${JSON.stringify(sensitiveAudit, null, 2)}\n`,
);

console.log(
  JSON.stringify(
    {
      outputRoot,
      inventory: inventory.summary,
      ignoreHardening: inventory.ignoreHardening,
      sensitiveAudit: {
        scannedTextFiles,
        scannedTextBytes,
        strongSecretFindingCount: strongSecretFindings.length,
        potentialCredentialAssignmentCount: potentialCredentialAssignments.length,
        localAbsolutePathFileCount: localPathExposure.length,
        localAbsolutePathOccurrences: sensitiveAudit.localAbsolutePathOccurrences,
      },
    },
    null,
    2,
  ),
);

if (inventory.summary.unclassified > 0 || strongSecretFindings.length > 0) {
  process.exitCode = 1;
}
