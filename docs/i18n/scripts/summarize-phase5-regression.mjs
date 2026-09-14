import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "../../..");
const auditRoot = path.join(repoRoot, "docs/i18n/audit/phase-5-release");
const logRoot = path.join(auditRoot, "logs");

const read = (relativePath) => fs.readFileSync(path.join(auditRoot, relativePath), "utf8");
const readJson = (relativePath) => JSON.parse(read(relativePath));

const i18nLog = read("logs/frontend-i18n-tests.log");
const libLog = read("logs/frontend-lib-tests.log");
const typeScriptLog = read("logs/typescript-check.log");
const nextLog = read("logs/next-production-build.log");
const rustLog = read("logs/rust-app-lib-tests.log");
const staticAudit = readJson("static-audit.json");
const runtimeAudit = readJson("runtime/runtime-audit.json");
const installAudit = readJson("windows-install-audit.json");
const integrityAudit = readJson("release-integrity.json");

function nodeTestCounts(log) {
  return {
    tests: Number(log.match(/# tests (\d+)/)?.[1] ?? -1),
    passed: Number(log.match(/# pass (\d+)/)?.[1] ?? -1),
    failed: Number(log.match(/# fail (\d+)/)?.[1] ?? -1),
  };
}

const rustMatch = rustLog.match(
  /test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;/,
);
const i18n = nodeTestCounts(i18nLog);
const frontendLib = nodeTestCounts(libLog);
const rust = {
  passed: Number(rustMatch?.[1] ?? -1),
  failed: Number(rustMatch?.[2] ?? -1),
  ignored: Number(rustMatch?.[3] ?? -1),
};

const results = {
  schemaVersion: 1,
  phase: "15.8-stage-5-chinese-qa-release-and-rollback",
  generatedAt: new Date().toISOString(),
  results: {
    frontendI18n: {
      command: "pnpm test:i18n",
      tests: i18n.tests,
      passedTests: i18n.passed,
      failedTests: i18n.failed,
      passed: i18n.tests === 54 && i18n.passed === 54 && i18n.failed === 0,
      log: path.relative(repoRoot, path.join(logRoot, "frontend-i18n-tests.log")),
    },
    frontendLibNodeCompatible: {
      command:
        "pnpm exec tsx --test <seven Node-compatible frontend/lib test files>",
      tests: frontendLib.tests,
      passedTests: frontendLib.passed,
      failedTests: frontendLib.failed,
      passed:
        frontendLib.tests === 41 && frontendLib.passed === 41 && frontendLib.failed === 0,
      excludedByKnownRunnerConstraint: [
        "frontend/tests/lib/blocknote-markdown.test.ts",
        "frontend/tests/lib/summary-language-preferences.test.js",
      ],
      exclusionReason: "Both files import bun:test; Bun is not installed in this audit environment.",
      log: path.relative(repoRoot, path.join(logRoot, "frontend-lib-tests.log")),
    },
    typeScript: {
      command: "pnpm exec tsc --noEmit",
      errors: typeScriptLog.trim() === "" ? 0 : null,
      passed: typeScriptLog.trim() === "",
      log: path.relative(repoRoot, path.join(logRoot, "typescript-check.log")),
    },
    nextProductionBuild: {
      command: "pnpm build",
      staticPages: Number(nextLog.match(/Generating static pages \((\d+)\/13\)/g)?.at(-1)?.match(/\((\d+)\/13\)/)?.[1] ?? -1),
      expectedStaticPages: 13,
      passed:
        nextLog.includes("✓ Compiled successfully") &&
        nextLog.includes("Generating static pages (13/13)"),
      log: path.relative(repoRoot, path.join(logRoot, "next-production-build.log")),
    },
    rustIntegration: {
      command: "cargo test -p meetily --test app_lib_tests -- --nocapture",
      passedTests: rust.passed,
      failedTests: rust.failed,
      ignoredTests: rust.ignored,
      passed: rust.passed === 278 && rust.failed === 0 && rust.ignored === 2,
      log: path.relative(repoRoot, path.join(logRoot, "rust-app-lib-tests.log")),
    },
    staticAudit: { passed: staticAudit.passed, checks: Object.keys(staticAudit.checks).length },
    runtimeAudit: { passed: runtimeAudit.passed, checks: Object.keys(runtimeAudit.assertions).length },
    windowsInstallAudit: { passed: installAudit.passed, checks: Object.keys(installAudit.assertions).length },
    releaseIntegrityAudit: { passed: integrityAudit.passed, checks: Object.keys(integrityAudit.assertions).length },
  },
};

results.automatedSuitePassed = Object.values(results.results).every((result) => result.passed);

const outputPath = path.join(auditRoot, "regression-summary.json");
fs.writeFileSync(outputPath, `${JSON.stringify(results, null, 2)}\n`);
console.log(JSON.stringify({ outputPath, automatedSuitePassed: results.automatedSuitePassed }, null, 2));

if (!results.automatedSuitePassed) {
  process.exitCode = 1;
}
