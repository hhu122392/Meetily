import assert from "node:assert/strict";
import test from "node:test";
import i18next from "i18next";
import { resources } from "../../src/i18n/resources";

type FlatResource = Map<string, string>;

function flatten(
  value: Record<string, unknown>,
  prefix = "",
  result: FlatResource = new Map(),
): FlatResource {
  for (const [key, child] of Object.entries(value)) {
    const next = prefix ? `${prefix}.${key}` : key;
    if (child !== null && typeof child === "object" && !Array.isArray(child)) {
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

test("phase 2A onboarding resources have identical keys and placeholders", () => {
  const en = flatten(resources.en.onboarding);
  const zhCN = flatten(resources["zh-CN"].onboarding);

  assert.deepEqual([...zhCN.keys()].sort(), [...en.keys()].sort());
  assert.ok(en.size >= 77);

  for (const [key, enValue] of en) {
    const zhValue = zhCN.get(key);
    if (!zhValue?.trim()) {
      assert.fail(`${key} must have a Simplified Chinese value`);
    }
    assert.deepEqual(
      placeholders(zhValue),
      placeholders(enValue),
      `${key} must preserve interpolation placeholders`,
    );
    assert.doesNotMatch(zhValue, /\b(?:TODO|TBD|TRANSLATE_ME)\b/i);
  }
});

test("phase 2A onboarding copy follows approved terminology and interpolates safely", async () => {
  const instance = i18next.createInstance();
  await instance.init({
    resources,
    lng: "zh-CN",
    fallbackLng: "en",
    ns: ["onboarding"],
    defaultNS: "onboarding",
    initAsync: false,
    interpolation: { escapeValue: false },
  });

  assert.equal(instance.t("labels.transcriptionEngine", { ns: "onboarding" }), "转录引擎");
  assert.equal(instance.t("labels.summaryEngine", { ns: "onboarding" }), "摘要引擎");
  assert.equal(instance.t("labels.microphone", { ns: "onboarding" }), "麦克风");
  assert.equal(instance.t("labels.systemAudio", { ns: "onboarding" }), "系统音频");
  assert.equal(
    instance.t("messages.stepWithTitle", {
      step: 2,
      title: instance.t("actions.downloadTranscriptionEngine", { ns: "onboarding" }),
      ns: "onboarding",
    }),
    "第 2 步：下载转录引擎",
  );
  assert.equal(
    instance.t("progress.downloaded", {
      downloaded: "12.5",
      total: "670.0",
      unit: "MiB",
      ns: "onboarding",
    }),
    "12.5 MiB / 670.0 MiB",
  );
});
