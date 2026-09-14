import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import i18next from "i18next";
import { resources } from "../../src/i18n/resources";

const repositoryRoot = path.resolve(process.cwd(), "..");
const namespaces = ["import", "transcription"] as const;

function read(relativePath: string): string {
  return fs.readFileSync(path.join(repositoryRoot, relativePath), "utf8");
}

function runPhase2Audit(batch: "2E" | "2F") {
  const temporaryDirectory = fs.mkdtempSync(path.join(os.tmpdir(), "meetily-phase2-audit-"));
  const reportPath = path.join(temporaryDirectory, "resource-and-source-audit.json");
  try {
    execFileSync(
      process.execPath,
      [
        path.join(repositoryRoot, "docs/i18n/scripts/audit-phase2.mjs"),
        repositoryRoot,
        `--batch=${batch}`,
        `--report=${reportPath}`,
      ],
      { cwd: repositoryRoot, stdio: "pipe" },
    );
    return JSON.parse(fs.readFileSync(reportPath, "utf8"));
  } finally {
    fs.rmSync(temporaryDirectory, { recursive: true, force: true });
  }
}

function flatten(value: Record<string, unknown>, prefix = "", result = new Map<string, string>()) {
  for (const [key, child] of Object.entries(value)) {
    const next = prefix ? `${prefix}.${key}` : key;
    if (child && typeof child === "object" && !Array.isArray(child)) {
      flatten(child as Record<string, unknown>, next, result);
    } else {
      assert.equal(typeof child, "string", `${next} must be a string`);
      result.set(next, child as string);
    }
  }
  return result;
}

function placeholders(value: string): string[] {
  return [...value.matchAll(/{{\s*([^},\s]+)[^}]*}}/g)].map((match) => match[1]).sort();
}

test("phase 2E resources are symmetric, translated, and placeholder-safe", () => {
  for (const namespace of namespaces) {
    const en = flatten(resources.en[namespace]);
    const zhCN = flatten(resources["zh-CN"][namespace]);
    assert.deepEqual([...zhCN.keys()].sort(), [...en.keys()].sort(), namespace);
    assert.ok(en.size >= (namespace === "import" ? 105 : 125), `${namespace} coverage`);
    for (const [key, enValue] of en) {
      const zhValue = zhCN.get(key);
      assert.ok(zhValue?.trim(), `${namespace}:${key} must have a Chinese value`);
      assert.deepEqual(placeholders(zhValue!), placeholders(enValue), `${namespace}:${key} placeholders`);
      assert.doesNotMatch(zhValue!, /\b(?:TODO|TBD|TRANSLATE_ME)\b/i);
    }
  }
});

test("phase 2E preserves every frozen English baseline key and value", () => {
  for (const namespace of namespaces) {
    const baseline = flatten(JSON.parse(read(`docs/i18n/baseline/locales/en/${namespace}.json`)));
    const current = flatten(resources.en[namespace]);
    for (const [key, value] of baseline) {
      assert.equal(current.get(key), value, `${namespace}:${key}`);
    }
  }
});

test("import, retranscription, and recovery labels switch locale without mutating business state", async () => {
  const instance = i18next.createInstance();
  await instance.init({
    resources,
    lng: "en",
    fallbackLng: "en",
    supportedLngs: ["en", "zh-CN"],
    ns: [...namespaces],
    defaultNS: "import",
    initAsync: false,
    interpolation: { escapeValue: false },
  });
  const t = instance.t.bind(instance) as (key: string, options?: Record<string, unknown>) => string;
  const state = {
    sourcePath: "D:/Audio/board-meeting.mp4",
    title: "Q3 Board Meeting",
    language: "ja",
    provider: "whisper",
    model: "large-v3-turbo",
    progress: 63,
    recoveryMeetingId: "meeting-recovery-42",
  };
  assert.equal(t("import:status.stages.transcribing"), "Transcribing speech");
  assert.equal(t("transcription:titles.retranscribeMeeting"), "Retranscribe Meeting");
  assert.equal(t("import:recovery.description", { count: 2 }), "We found 2 interrupted meetings. Select a meeting to preview and recover it.");
  await instance.changeLanguage("zh-CN");
  assert.equal(t("import:status.stages.transcribing"), "正在转写语音");
  assert.equal(t("transcription:titles.retranscribeMeeting"), "重新转写会议");
  assert.equal(t("import:recovery.description", { count: 2 }), "发现 2 个中断的会议。请选择一个会议进行预览和恢复。");
  assert.deepEqual(state, {
    sourcePath: "D:/Audio/board-meeting.mp4", title: "Q3 Board Meeting", language: "ja",
    provider: "whisper", model: "large-v3-turbo", progress: 63,
    recoveryMeetingId: "meeting-recovery-42",
  });
});

test("all backend progress stages resolve through controlled localization keys", () => {
  const importEn = flatten(resources.en.import);
  const transcriptionEn = flatten(resources.en.transcription);
  for (const stage of ["copying", "decoding", "resampling", "vad", "transcribing", "saving", "complete", "unknown"]) {
    assert.ok(importEn.has(`status.stages.${stage}`), stage);
  }
  for (const stage of ["decoding", "vad", "transcribing", "saving", "complete", "unknown"]) {
    assert.ok(transcriptionEn.has(`status.retranscriptionStages.${stage}`), stage);
  }
  const importDialog = read("frontend/src/components/ImportAudio/ImportAudioDialog.tsx");
  const retranscribeDialog = read("frontend/src/components/MeetingDetails/RetranscribeDialog.tsx");
  assert.doesNotMatch(importDialog, /progress\.message/);
  assert.doesNotMatch(retranscribeDialog, /progress\.message/);
  assert.match(importDialog, /status\.stages\.\$\{progress\.stage\}/);
  assert.match(retranscribeDialog, /status\.retranscriptionStages\.\$\{progress\.stage\}/);
});

test("raw backend errors and recovery messages never reach user-visible surfaces", () => {
  const visibleSources = [
    "frontend/src/components/ImportAudio/ImportAudioDialog.tsx",
    "frontend/src/components/MeetingDetails/RetranscribeDialog.tsx",
    "frontend/src/components/TranscriptRecovery/TranscriptRecovery.tsx",
    "frontend/src/components/DatabaseImport/HomebrewDatabaseDetector.tsx",
    "frontend/src/components/DatabaseImport/LegacyDatabaseImport.tsx",
  ];
  for (const relativePath of visibleSources) {
    const source = read(relativePath);
    assert.doesNotMatch(source, /toast\.error\(\s*(?:err|error|errorMsg|errorMessage)\b/i, relativePath);
    assert.doesNotMatch(source, /description\s*:\s*(?:String\s*\(|(?:err|error|errorMsg|errorMessage)(?:\.message)?\b)/i, relativePath);
    assert.doesNotMatch(source, />\s*\{\s*(?:event\.payload\.(?:error|message)|progress\.message|errorMessage)\s*\}\s*</i, relativePath);
    assert.doesNotMatch(source, /alert\(/, relativePath);
  }
  const importHook = read("frontend/src/hooks/useImportAudio.ts");
  const recoveryHook = read("frontend/src/hooks/useTranscriptRecovery.ts");
  assert.doesNotMatch(importHook, /setError\(event\.payload\.error\)/);
  assert.doesNotMatch(importHook, /onErrorRef\.current\?\.\(event\.payload\.error\)/);
  assert.doesNotMatch(recoveryHook, /toast\.(?:error|warning)\([^\n]*(?:message|errorMsg|String\()/i);
});

test("cancel failures preserve active import and retranscription state", () => {
  const importHook = read("frontend/src/hooks/useImportAudio.ts");
  const importDialog = read("frontend/src/components/ImportAudio/ImportAudioDialog.tsx");
  const retranscribeDialog = read("frontend/src/components/MeetingDetails/RetranscribeDialog.tsx");
  assert.match(importHook, /cancelImport:\s*\(\)\s*=>\s*Promise<boolean>/);
  assert.match(importHook, /isCancelledRef\.current = false;\s*return false;/);
  assert.match(importDialog, /const cancelled = await cancelImport\(\);\s*if \(!cancelled\) \{[\s\S]*?return;/);
  assert.match(retranscribeDialog, /catch \(err\) \{[\s\S]*?cancelRetranscriptionFailed[\s\S]*?return;/);
  assert.doesNotMatch(retranscribeDialog, /catch \(err\) \{[\s\S]*?\}\s*onOpenChange\(false\);/);
});

test("recovery keeps recoverable data until the backend save succeeds and confirms deletion", () => {
  const hook = read("frontend/src/hooks/useTranscriptRecovery.ts");
  const dialog = read("frontend/src/components/TranscriptRecovery/TranscriptRecovery.tsx");
  const saveIndex = hook.indexOf("storageService.saveMeeting");
  const markSavedIndex = hook.indexOf("indexedDBService.markMeetingSaved");
  const cleanupIndex = hook.indexOf("cleanup_checkpoints");
  assert.ok(saveIndex >= 0 && markSavedIndex > saveIndex, "IndexedDB recovery data must remain until save succeeds");
  assert.ok(cleanupIndex > markSavedIndex, "audio checkpoints must only be cleaned after the meeting is marked saved");
  assert.match(dialog, /window\.confirm\(t\('confirmations\.deleteMeeting'\)\)/);
  assert.match(dialog, /setActionError\('deleteMeetingFailed'\)/);
  assert.match(dialog, /if \(!open && !isRecovering && !isDeleting\) onClose\(\)/);
});

test("phase 2E machine audit is clean", () => {
  const audit = runPhase2Audit("2E");
  assert.equal(audit.batch, "2E");
  assert.deepEqual(audit.summary, {
    resourceFailures: 0,
    sourceFailures: 0,
    totalFailures: 0,
    passed: true,
  });
});
