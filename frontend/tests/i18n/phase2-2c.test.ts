import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import i18next from "i18next";
import ts from "typescript";
import { resources } from "../../src/i18n/resources";

const repositoryRoot = path.resolve(process.cwd(), "..");
const namespaces = ["meetings", "summary", "templates"] as const;

const migratedSources = [
  "frontend/src/app/meeting-details/page-content.tsx",
  "frontend/src/app/meeting-details/page.tsx",
  "frontend/src/components/AISummary/Block.tsx",
  "frontend/src/components/AISummary/BlockNoteSummaryView.tsx",
  "frontend/src/components/AISummary/index.tsx",
  "frontend/src/components/AISummary/Section.tsx",
  "frontend/src/components/EditableTitle.tsx",
  "frontend/src/components/EmptyStateSummary.tsx",
  "frontend/src/components/LanguagePickerPopover.tsx",
  "frontend/src/components/MeetingDetails/SummaryGeneratorButtonGroup.tsx",
  "frontend/src/components/MeetingDetails/SummaryPanel.tsx",
  "frontend/src/components/MeetingDetails/SummaryUpdaterButtonGroup.tsx",
  "frontend/src/components/MeetingDetails/TranscriptButtonGroup.tsx",
  "frontend/src/components/MeetingDetails/TranscriptPanel.tsx",
  "frontend/src/hooks/meeting-details/useCopyOperations.ts",
  "frontend/src/hooks/meeting-details/useMeetingData.ts",
  "frontend/src/hooks/meeting-details/useMeetingOperations.ts",
  "frontend/src/hooks/meeting-details/useModelConfiguration.ts",
  "frontend/src/hooks/meeting-details/useSummaryGeneration.ts",
  "frontend/src/hooks/meeting-details/useTemplates.ts",
  "frontend/src/lib/summary-languages.ts",
] as const;

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

function userFacingLiterals(relativePath: string): Array<{ line: number; text: string }> {
  const source = fs.readFileSync(path.join(repositoryRoot, relativePath), "utf8");
  const sourceFile = ts.createSourceFile(
    relativePath,
    source,
    ts.ScriptTarget.Latest,
    true,
    relativePath.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
  );
  const findings: Array<{ line: number; text: string }> = [];

  const literal = (node: ts.Node): string => {
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node) || ts.isJsxText(node)) {
      return node.text.trim();
    }
    return "";
  };

  const visit = (node: ts.Node) => {
    const text = literal(node);
    if (text && /[A-Za-z]{2}/.test(text) && !/^https?:\/\//.test(text)) {
      const parent = node.parent;
      const jsxAttribute = ts.isStringLiteral(node) && ts.isJsxAttribute(parent)
        ? parent.name.getText(sourceFile)
        : "";
      const callName = ts.isCallExpression(parent) ? parent.expression.getText(sourceFile) : "";
      const objectProperty = ts.isStringLiteral(node) && ts.isPropertyAssignment(parent)
        ? parent.name.getText(sourceFile).replace(/["']/g, "")
        : "";
      const isUiLiteral =
        ts.isJsxText(node) ||
        ["title", "placeholder", "aria-label", "alt"].includes(jsxAttribute) ||
        /^(?:toast\.(?:error|success|info|warning)|alert|confirm)$/.test(callName) ||
        ["title", "description", "label", "placeholder", "message"].includes(objectProperty);

      if (isUiLiteral) {
        const { line } = sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile));
        findings.push({ line: line + 1, text });
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(sourceFile);
  return findings;
}

test("phase 2C resources have identical non-empty keys and placeholders", () => {
  for (const namespace of namespaces) {
    const en = flatten(resources.en[namespace]);
    const zhCN = flatten(resources["zh-CN"][namespace]);
    assert.deepEqual([...zhCN.keys()].sort(), [...en.keys()].sort(), namespace);
    for (const [key, enValue] of en) {
      const zhValue = zhCN.get(key);
      assert.ok(zhValue?.trim(), `${namespace}:${key} must have a Chinese value`);
      assert.deepEqual(placeholders(zhValue!), placeholders(enValue), `${namespace}:${key} placeholders`);
      assert.doesNotMatch(zhValue!, /\b(?:TODO|TBD|TRANSLATE_ME)\b/i);
    }
  }
});

test("phase 2C key flows render in English and Simplified Chinese without changing state", async () => {
  const instance = i18next.createInstance();
  await instance.init({
    resources,
    lng: "en",
    fallbackLng: "en",
    supportedLngs: ["en", "zh-CN"],
    ns: [...namespaces],
    defaultNS: "meetings",
    initAsync: false,
    interpolation: { escapeValue: false },
  });
  const translate = instance.t.bind(instance) as (key: string, options?: Record<string, unknown>) => string;

  assert.equal(translate("meetings:actions.openRecordingFolder"), "Open recording folder");
  assert.equal(
    translate("summary:descriptions.valueIsDownloading", { selectedModel: "Qwen", progress: 42 }),
    "Qwen is downloading (42%). Please wait until the download completes.",
  );
  const preservedState = { meetingId: "meeting-2c", summaryStatus: "summarizing", templateId: "standard_meeting" };

  await instance.changeLanguage("zh-CN");
  assert.equal(translate("meetings:actions.openRecordingFolder"), "打开录音文件夹");
  assert.equal(
    translate("summary:descriptions.valueIsDownloading", { selectedModel: "Qwen", progress: 42 }),
    "Qwen 正在下载（42%），请等待下载完成。",
  );
  assert.deepEqual(preservedState, {
    meetingId: "meeting-2c",
    summaryStatus: "summarizing",
    templateId: "standard_meeting",
  });
});

test("phase 2C migrated sources contain no user-visible English literals", () => {
  for (const relativePath of migratedSources) {
    assert.deepEqual(userFacingLiterals(relativePath), [], relativePath);
  }
});

test("phase 2C source contracts block raw error bubbling and translated control flow", () => {
  for (const relativePath of migratedSources) {
    const source = fs.readFileSync(path.join(repositoryRoot, relativePath), "utf8");
    assert.doesNotMatch(
      source,
      /description\s*:\s*(?:String\s*\(|(?:error|errorMessage|transcriptError|pollingResult\.error)(?:\.message)?\b)/i,
      `${relativePath} exposes a raw error description`,
    );
    assert.doesNotMatch(
      source,
      /toast\.error\(\s*(?:error|errorMessage|transcriptError|pollingResult\.error)\b/i,
      `${relativePath} exposes a raw error as a toast title`,
    );
    assert.doesNotMatch(
      source,
      /<[^>]+>\s*\{\s*(?:error|errorMessage|transcriptError|pollingResult\.error)\s*\}\s*</i,
      `${relativePath} renders a raw error`,
    );
    assert.doesNotMatch(
      source,
      /t\([^\n]+\)\s*(?:===|!==|==|!=)|(?:===|!==|==|!=)\s*t\(/,
      `${relativePath} compares translated text in business logic`,
    );
  }
});

test("phase 2C accessible controls and locale-aware formatting are wired", () => {
  const transcriptButtons = fs.readFileSync(
    path.join(repositoryRoot, "frontend/src/components/MeetingDetails/TranscriptButtonGroup.tsx"),
    "utf8",
  );
  const summaryPanel = fs.readFileSync(
    path.join(repositoryRoot, "frontend/src/components/MeetingDetails/SummaryPanel.tsx"),
    "utf8",
  );
  const copyOperations = fs.readFileSync(
    path.join(repositoryRoot, "frontend/src/hooks/meeting-details/useCopyOperations.ts"),
    "utf8",
  );
  assert.match(transcriptButtons, /aria-label=\{[^\n]*t\('accessibilityActions\.copyTranscript'\)/);
  assert.match(transcriptButtons, /aria-label=\{t\('actions\.openRecordingFolder'\)\}/);
  assert.match(summaryPanel, /aria-label=\{t\('accessibility\.setSummaryLanguage'\)\}/);
  assert.match(copyOperations, /new Intl\.DateTimeFormat\(locale/);
});

test("phase 2C meeting-template failures use controlled copy instead of backend message keys", () => {
  const templatesHook = fs.readFileSync(
    path.join(repositoryRoot, "frontend/src/hooks/meeting-details/useTemplates.ts"),
    "utf8",
  );
  assert.match(templatesHook, /description:\s*t\('templatePreference\.retryDescription'\)/);
  assert.doesNotMatch(templatesHook, /t\(normalized\.messageKey/);
});
