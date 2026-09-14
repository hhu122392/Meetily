import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import i18next from "i18next";
import ts from "typescript";
import { resources } from "../../src/i18n/resources";

const repositoryRoot = path.resolve(process.cwd(), "..");
const namespaces = ["settings", "models"] as const;
const migratedSources = [
  "frontend/src/app/settings/page.tsx",
  "frontend/src/app/_components/SettingsModal.tsx",
  "frontend/src/components/SettingTabs.tsx",
  "frontend/src/components/PreferenceSettings.tsx",
  "frontend/src/components/ModelSettingsModal.tsx",
  "frontend/src/components/SummaryModelSettings.tsx",
  "frontend/src/components/SummaryLanguageSettings.tsx",
  "frontend/src/components/RecordingSettings.tsx",
  "frontend/src/components/TranscriptSettings.tsx",
  "frontend/src/components/DeviceSelection.tsx",
  "frontend/src/components/AudioBackendSelector.tsx",
  "frontend/src/components/AudioLevelMeter.tsx",
  "frontend/src/components/BuiltInModelManager.tsx",
  "frontend/src/components/ModelDownloadProgress.tsx",
  "frontend/src/components/ParakeetModelManager.tsx",
  "frontend/src/components/WhisperModelManager.tsx",
] as const;

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

function placeholders(value: string) {
  return [...value.matchAll(/{{\s*([^},\s]+)[^}]*}}/g)].map((match) => match[1]).sort();
}

function userFacingLiterals(relativePath: string) {
  const source = fs.readFileSync(path.join(repositoryRoot, relativePath), "utf8");
  const sourceFile = ts.createSourceFile(
    relativePath,
    source,
    ts.ScriptTarget.Latest,
    true,
    relativePath.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
  );
  const findings: Array<{ line: number; text: string }> = [];
  const literal = (node: ts.Node) =>
    ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node) || ts.isJsxText(node)
      ? node.text.trim()
      : "";
  const visit = (node: ts.Node) => {
    const text = literal(node);
    if (text && /[A-Za-z]{2}/.test(text) && !/^https?:\/\//.test(text)) {
      if (/^(?:bg|text|border|ring|fill|stroke|from|to|via)-[a-z]+-\d{2,3}$/.test(text)) {
        ts.forEachChild(node, visit);
        return;
      }
      const parent = node.parent;
      const attribute = ts.isStringLiteral(node) && ts.isJsxAttribute(parent) ? parent.name.getText(sourceFile) : "";
      const callName = ts.isCallExpression(parent) ? parent.expression.getText(sourceFile) : "";
      const property = ts.isStringLiteral(node) && ts.isPropertyAssignment(parent)
        ? parent.name.getText(sourceFile).replace(/["']/g, "")
        : "";
      const translated = ts.isCallExpression(parent) && parent.expression.getText(sourceFile) === "t";
      const visible = !translated && (
        ts.isJsxText(node) ||
        ["title", "placeholder", "aria-label", "alt"].includes(attribute) ||
        /^(?:toast\.(?:error|success|info|warning)|alert|confirm)$/.test(callName) ||
        ["title", "description", "label", "placeholder", "message"].includes(property)
      );
      if (visible) {
        findings.push({ line: sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile)).line + 1, text });
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(sourceFile);
  return findings;
}

test("phase 2D resources are complete, symmetric, translated, and placeholder-safe", () => {
  for (const namespace of namespaces) {
    const en = flatten(resources.en[namespace]);
    const zhCN = flatten(resources["zh-CN"][namespace]);
    assert.deepEqual([...zhCN.keys()].sort(), [...en.keys()].sort(), namespace);
    assert.ok(en.size >= (namespace === "settings" ? 140 : 120), `${namespace} resource coverage`);
    for (const [key, enValue] of en) {
      const zhValue = zhCN.get(key);
      assert.ok(zhValue?.trim(), `${namespace}:${key} must have a Chinese value`);
      assert.deepEqual(placeholders(zhValue!), placeholders(enValue), `${namespace}:${key} placeholders`);
      assert.doesNotMatch(zhValue!, /\b(?:TODO|TBD|TRANSLATE_ME)\b/i);
    }
  }
});

test("phase 2D preserves every frozen English baseline key and value", () => {
  for (const namespace of namespaces) {
    const baseline = flatten(JSON.parse(fs.readFileSync(
      path.join(repositoryRoot, `docs/i18n/baseline/locales/en/${namespace}.json`),
      "utf8",
    )));
    const current = flatten(resources.en[namespace]);
    for (const [key, value] of baseline) {
      assert.equal(current.get(key), value, `${namespace}:${key}`);
    }
  }
});

test("settings, model, download, and device flows switch language without mutating business state", async () => {
  const instance = i18next.createInstance();
  await instance.init({
    resources,
    lng: "en",
    fallbackLng: "en",
    supportedLngs: ["en", "zh-CN"],
    ns: [...namespaces],
    defaultNS: "settings",
    initAsync: false,
    interpolation: { escapeValue: false },
  });
  const t = instance.t.bind(instance) as (key: string, options?: Record<string, unknown>) => string;
  const state = {
    provider: "ollama",
    model: "qwen3.5:2b",
    apiKey: "secret-value",
    micDevice: "Microphone Array (input)",
    systemDevice: "Speakers (output)",
    backend: "screencapturekit",
    saveFolder: "D:/Meetily recordings",
  };
  assert.equal(t("settings:page.title"), "Settings");
  assert.equal(t("models:status.downloadingModelEllipsis", { model: state.model }), `Downloading ${state.model}...`);
  assert.equal(t("settings:devices.selectionDescription", state), "Microphone: Microphone Array (input), System audio: Speakers (output)");
  await instance.changeLanguage("zh-CN");
  assert.equal(t("settings:page.title"), "设置");
  assert.equal(t("models:status.downloadingModelEllipsis", { model: state.model }), `正在下载 ${state.model}…`);
  assert.equal(t("settings:devices.selectionDescription", state), "麦克风：Microphone Array (input)，系统音频：Speakers (output)");
  assert.deepEqual(state, {
    provider: "ollama", model: "qwen3.5:2b", apiKey: "secret-value",
    micDevice: "Microphone Array (input)", systemDevice: "Speakers (output)",
    backend: "screencapturekit", saveFolder: "D:/Meetily recordings",
  });
});

test("phase 2D migrated sources contain no user-visible English literals", () => {
  for (const relativePath of migratedSources) {
    assert.deepEqual(userFacingLiterals(relativePath), [], relativePath);
  }
});

test("phase 2D blocks raw backend errors and secrets from UI surfaces", () => {
  for (const relativePath of migratedSources) {
    const source = fs.readFileSync(path.join(repositoryRoot, relativePath), "utf8");
    assert.doesNotMatch(source, /toast\.error\(\s*(?:err|error|errorMsg|errorMessage)\b/i, relativePath);
    assert.doesNotMatch(source, /description\s*:\s*(?:String\s*\(|(?:err|error|errorMsg|errorMessage)(?:\.message)?\b)/i, relativePath);
    assert.doesNotMatch(source, /<[^>]+>\s*\{\s*(?:err|error|errorMsg|errorMessage)\s*\}\s*</i, relativePath);
    assert.doesNotMatch(source, /toast\.(?:error|success|info)\([^\n]*(?:apiKey|customOpenAIApiKey)/i, relativePath);
    assert.doesNotMatch(source, /t\([^\n]+\)\s*(?:===|!==|==|!=)|(?:===|!==|==|!=)\s*t\(/, relativePath);
  }
  const modelSettings = fs.readFileSync(path.join(repositoryRoot, "frontend/src/components/ModelSettingsModal.tsx"), "utf8");
  assert.doesNotMatch(modelSettings, /toast\.success\(\s*result\.message/);
  const backend = fs.readFileSync(path.join(repositoryRoot, "frontend/src/components/AudioBackendSelector.tsx"), "utf8");
  assert.doesNotMatch(backend, />\s*\{backend\.description\}\s*</);
});

test("phase 2D accessibility and live-locale event contracts are wired", () => {
  const devices = fs.readFileSync(path.join(repositoryRoot, "frontend/src/components/DeviceSelection.tsx"), "utf8");
  const transcript = fs.readFileSync(path.join(repositoryRoot, "frontend/src/components/TranscriptSettings.tsx"), "utf8");
  const modelSettings = fs.readFileSync(path.join(repositoryRoot, "frontend/src/components/ModelSettingsModal.tsx"), "utf8");
  const parakeet = fs.readFileSync(path.join(repositoryRoot, "frontend/src/components/ParakeetModelManager.tsx"), "utf8");
  const whisper = fs.readFileSync(path.join(repositoryRoot, "frontend/src/components/WhisperModelManager.tsx"), "utf8");
  const recording = fs.readFileSync(path.join(repositoryRoot, "frontend/src/components/RecordingSettings.tsx"), "utf8");
  const preferences = fs.readFileSync(path.join(repositoryRoot, "frontend/src/components/PreferenceSettings.tsx"), "utf8");
  const summary = fs.readFileSync(path.join(repositoryRoot, "frontend/src/components/SummaryModelSettings.tsx"), "utf8");
  assert.match(devices, /aria-label=\{t\('devices\.refresh'\)\}/);
  assert.match(transcript, /aria-label=\{showApiKey \? t\('actions\.hideApiKey'\) : t\('actions\.showApiKey'\)\}/);
  assert.match(modelSettings, /aria-label=\{showApiKey \? t\('actions\.hideApiKey'\) : t\('actions\.showApiKey'\)\}/);
  assert.match(parakeet, /\}, \[localizedModelName, t\]\);/);
  assert.match(whisper, /\}, \[t\]\);/);
  assert.match(recording, /const previousPreferences = preferences;/);
  assert.match(recording, /if \(!saved\) \{\s*setPreferences\(previousPreferences\);\s*return;/);
  assert.match(recording, /savePreferences = async \(prefs: RecordingPreferences\): Promise<boolean>/);
  assert.match(recording, /aria-label=\{t\('recording\.saveAudio'\)\}/);
  assert.match(recording, /aria-label=\{t\('recording\.startNotification'\)\}/);
  assert.match(preferences, /aria-label=\{t\("preferences\.notificationsTitle"\)\}/);
  assert.match(summary, /aria-label=\{t\('summary\.autoTitle'\)\}/);
});
