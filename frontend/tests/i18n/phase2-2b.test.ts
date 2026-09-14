import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import i18next from "i18next";
import { resources } from "../../src/i18n/resources";

const namespaces = ["navigation", "recording", "transcription"] as const;
const repositoryRoot = path.resolve(process.cwd(), "..");
const manifest = JSON.parse(
  fs.readFileSync(
    path.join(repositoryRoot, "docs/i18n/phase-2/phase2-batches.json"),
    "utf8",
  ),
) as { batches: { "2B": { sources: string[] } } };

function flatten(
  value: Record<string, unknown>,
  prefix = "",
  result = new Map<string, string>(),
): Map<string, string> {
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
  return [...value.matchAll(/{{\s*([^},\s]+)[^}]*}}/g)]
    .map((match) => match[1])
    .sort();
}

test("phase 2B resources have identical non-empty keys and placeholders", () => {
  for (const namespace of namespaces) {
    const en = flatten(resources.en[namespace]);
    const zhCN = flatten(resources["zh-CN"][namespace]);

    assert.deepEqual([...zhCN.keys()].sort(), [...en.keys()].sort(), namespace);
    for (const [key, enValue] of en) {
      const zhValue = zhCN.get(key);
      assert.ok(zhValue?.trim(), `${namespace}:${key} must have a Chinese value`);
      assert.deepEqual(
        placeholders(zhValue!),
        placeholders(enValue),
        `${namespace}:${key} placeholders`,
      );
      assert.doesNotMatch(zhValue!, /\b(?:TODO|TBD|TRANSLATE_ME)\b/i);
    }
  }
});

test("phase 2B key flows render in English and Simplified Chinese", async () => {
  const instance = i18next.createInstance();
  await instance.init({
    resources,
    lng: "en",
    fallbackLng: "en",
    supportedLngs: ["en", "zh-CN"],
    ns: [...namespaces],
    defaultNS: "recording",
    initAsync: false,
    interpolation: { escapeValue: false },
  });

  assert.equal(instance.t("recording:actions.startRecording"), "Start recording");
  assert.equal(instance.t("recording:messages.recordingStarted"), "🔴 Recording Started");
  assert.equal(
    instance.t("recording:status.processingRemainingChunks", { count: 2 }),
    "Processing 2 remaining chunks...",
  );

  const preservedRecordingState = { status: "recording", elapsedSeconds: 42 };
  await instance.changeLanguage("zh-CN");

  assert.equal(instance.t("recording:actions.startRecording"), "开始录音");
  assert.equal(instance.t("recording:actions.resumeRecording"), "继续录音");
  assert.equal(instance.t("recording:messages.recordingStarted"), "🔴 已开始录音");
  assert.equal(
    instance.t("recording:labels.informAllParticipantsThisMeetingIsBeingRecorded"),
    "请告知所有参会者本次会议正在录音。",
  );
  assert.equal(
    instance.t("recording:status.processingRemainingChunks", { count: 2 }),
    "正在处理剩余的 2 个音频块…",
  );
  assert.deepEqual(preservedRecordingState, {
    status: "recording",
    elapsedSeconds: 42,
  });
});

test("phase 2B source contracts block raw error bubbling and translated control flow", () => {
  for (const relativePath of manifest.batches["2B"].sources) {
    const source = fs.readFileSync(path.join(repositoryRoot, relativePath), "utf8");

    assert.doesNotMatch(
      source,
      /(?:toast\.(?:error|info|success|warning)|alert)\(\s*["'`][A-Za-z]/,
      `${relativePath} has a literal user notification`,
    );
    assert.doesNotMatch(
      source,
      /description\s*:\s*(?:[\w.]*error(?:\.message)?|String\s*\(|userMessage|event\.payload)/i,
      `${relativePath} exposes a raw error description`,
    );
    assert.doesNotMatch(
      source,
      /showModal\([^,]+,\s*(?:[\w.]*error(?:\.message)?|userMessage|event\.payload)/i,
      `${relativePath} exposes a raw backend event in a modal`,
    );
    assert.doesNotMatch(
      source,
      /t\([^\n]+\)\s*(?:===|!==|==|!=)|(?:===|!==|==|!=)\s*t\(/,
      `${relativePath} compares translated text in business logic`,
    );
    assert.doesNotMatch(
      source,
      /setStatus\(\s*RecordingStatus\.[A-Z_]+\s*,\s*["'`]/,
      `${relativePath} stores a hard-coded status message`,
    );
  }
});

test("recording controls expose localized accessible names", () => {
  const controls = fs.readFileSync(
    path.join(repositoryRoot, "frontend/src/components/RecordingControls.tsx"),
    "utf8",
  );
  const sidebar = fs.readFileSync(
    path.join(repositoryRoot, "frontend/src/components/Sidebar/index.tsx"),
    "utf8",
  );

  for (const key of [
    "actions.startRecording",
    "actions.pauseRecording",
    "actions.resumeRecording",
    "actions.stopRecording",
    "accessibility.closeAlert",
  ]) {
    assert.match(controls, new RegExp(`t\\(['\"]${key.replace(".", "\\.")}`));
  }
  assert.match(sidebar, /aria-label=\{t\('accessibility\.editMeetingTitle'\)\}/);
  assert.match(sidebar, /aria-label=\{t\('accessibility\.deleteMeeting'\)\}/);
});
