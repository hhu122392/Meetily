import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "../../..");
const auditRoot = path.join(repoRoot, "docs/i18n/audit/phase-5-release");
const readJson = (relativePath) =>
  JSON.parse(fs.readFileSync(path.join(auditRoot, relativePath), "utf8"));

const technicalAudits = {
  regression: readJson("regression-summary.json").automatedSuitePassed,
  static: readJson("static-audit.json").passed,
  runtime: readJson("runtime/runtime-audit.json").passed,
  windowsInstall: readJson("windows-install-audit.json").passed,
  artifactIntegrity: readJson("release-integrity.json").passed,
};

const blockers = [
  {
    id: "P5-UPSTREAM-001",
    area: "prior-stage-approval",
    status: "OPEN",
    reason: "Stage 2, 3, and 4 conditional items are not all closed or formally accepted.",
  },
  {
    id: "P5-GOV-001",
    area: "language-signoff",
    status: "OPEN",
    reason: "The required second Chinese language reviewer has not signed.",
  },
  {
    id: "P5-GOV-002",
    area: "privacy-legal-signoff",
    status: "OPEN",
    reason: "The Chinese privacy/legal text review has not been signed by an authorized reviewer.",
  },
  {
    id: "P5-SIGN-001",
    area: "code-signing",
    status: "OPEN",
    reason: "The audit candidate, NSIS installer, and installed payload are NotSigned.",
  },
  {
    id: "P5-PLAT-001",
    area: "supported-platform-matrix",
    status: "OPEN",
    reason: "macOS/Linux evidence is absent and a Windows-only release scope is not approved.",
  },
  {
    id: "P5-HW-001",
    area: "long-running-recording",
    status: "OPEN",
    reason: "A real 60-minute recording with pause/resume/locale-switch/stop/save was not performed.",
  },
  {
    id: "P5-AI-001",
    area: "real-model-output",
    status: "OPEN",
    reason: "Real multi-provider English/Chinese summary outputs have no human QA evidence.",
  },
  {
    id: "P5-UPG-001",
    area: "upgrade-and-version-rollback",
    status: "OPEN",
    reason: "A real prior-version in-place upgrade and approved package rollback were not performed.",
  },
  {
    id: "P5-PERF-001",
    area: "cold-start-baseline",
    status: "OPEN",
    reason: "No pre-i18n English cold-start baseline exists for the <=10% regression gate.",
  },
  {
    id: "P5-A11Y-001",
    area: "assistive-technology",
    status: "OPEN",
    reason: "Narrator/NVDA and keyboard-only human verification were not performed.",
  },
  {
    id: "P5-NATIVE-001",
    area: "windows-shell-native-surfaces",
    status: "OPEN",
    reason: "Windows tray states and notifications do not have bilingual OS Shell screenshots and human sign-off for this audit round.",
  },
  {
    id: "P5-REL-001",
    area: "release-traceability",
    status: "OPEN",
    reason: "The isolated candidate was built from a shared dirty worktree and cannot be mapped to a clean, reproducible release commit.",
  },
];

const limitations = [
  {
    id: "P5-TOOL-001",
    area: "test-runner-coverage",
    status: "OPEN",
    reason: "Two existing bun:test files were not executed because Bun is unavailable in this audit environment.",
  },
  {
    id: "P5-TOOL-002",
    area: "lint-gate",
    status: "OPEN",
    reason: "The repository has no reproducible non-interactive ESLint gate for this audit environment.",
  },
];

const technicalSuitePassed = Object.values(technicalAudits).every(Boolean);
const openBlockers = blockers.filter((blocker) => blocker.status === "OPEN");
const releaseGatePassed = technicalSuitePassed && openBlockers.length === 0;

const report = {
  schemaVersion: 1,
  phase: "15.8-stage-5-chinese-qa-release-and-rollback",
  generatedAt: new Date().toISOString(),
  technicalSuitePassed,
  technicalAudits,
  releaseGatePassed,
  verdict: releaseGatePassed ? "PASS" : "FAIL",
  formalReleaseAuthorized: releaseGatePassed,
  openBlockerCount: openBlockers.length,
  blockers,
  openLimitationCount: limitations.filter((limitation) => limitation.status === "OPEN").length,
  limitations,
  policy:
    "Stage 5 must be FAIL when any mandatory signoff, platform/device, real upgrade/rollback, long-recording, model-output, accessibility, native-shell, performance-baseline, or clean-release-traceability evidence is absent.",
};

const outputPath = path.join(auditRoot, "release-gate.json");
fs.writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`);
console.log(
  JSON.stringify(
    {
      outputPath,
      technicalSuitePassed,
      releaseGatePassed,
      verdict: report.verdict,
      openBlockerCount: report.openBlockerCount,
    },
    null,
    2,
  ),
);

// A FAIL verdict is the correct audited outcome while blockers remain, not a script failure.
