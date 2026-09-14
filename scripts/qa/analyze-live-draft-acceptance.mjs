import fs from "node:fs";
import path from "node:path";

const [monitorPath, playbackStartedAt, outputPath] = process.argv.slice(2);
if (!monitorPath || !playbackStartedAt || !outputPath) {
  throw new Error(
    "Usage: node analyze-live-draft-acceptance.mjs <monitor.json> <playback-started-at-iso> <output.json>",
  );
}

const reference =
  "今天进行 Meetily 核心功能验收。会议时间是二〇二六年八月二十四日十九点三十分，会议主题是中文录音、转写与摘要闭环。参会人包括李明、王芳和赵强，项目代号是 M100。第一项结论：本周五前完成简体中文转写验证。第二项结论：摘要必须使用已经绑定的会议模板。第一项行动：王芳在八月二十八日前提交真实测试报告。第二项行动：赵强负责修复摘要生成失败和重复报错。当前风险包括录音延迟、整段漏转、繁体字输出和摘要生成失败。下面暂停录音十秒，恢复后继续。";

const monitor = JSON.parse(fs.readFileSync(monitorPath, "utf8"));
const playbackEpochMs = Date.parse(playbackStartedAt);
if (!Number.isFinite(playbackEpochMs)) throw new Error("Invalid playback start timestamp");

const normalize = (value) =>
  value
    .normalize("NFKC")
    .toLocaleLowerCase("en-US")
    .replace(/[\p{P}\p{S}\p{Z}\s]/gu, "");

function levenshtein(referenceText, hypothesisText) {
  const left = [...referenceText];
  const right = [...hypothesisText];
  let previous = Array.from({ length: right.length + 1 }, (_, index) => index);
  for (let i = 1; i <= left.length; i += 1) {
    const current = [i];
    for (let j = 1; j <= right.length; j += 1) {
      current[j] = Math.min(
        previous[j] + 1,
        current[j - 1] + 1,
        previous[j - 1] + (left[i - 1] === right[j - 1] ? 0 : 1),
      );
    }
    previous = current;
  }
  return previous[right.length];
}

const nonempty = monitor.changes.filter(
  (change) => change.segmentCount > 0 && String(change.text ?? "").trim(),
);
const finalLive = nonempty.at(-1) ?? null;
const hypothesis = finalLive?.text ?? "";
const normalizedReference = normalize(reference);
const normalizedHypothesis = normalize(hypothesis);
const editDistance = levenshtein(normalizedReference, normalizedHypothesis);
const cer = editDistance / normalizedReference.length;
const firstVisibleMs = nonempty.length
  ? Date.parse(nonempty[0].at) - playbackEpochMs
  : null;
const updateGapsMs = nonempty.slice(1).map(
  (change, index) => Date.parse(change.at) - Date.parse(nonempty[index].at),
);
const maxUpdateGapMs = updateGapsMs.length ? Math.max(...updateGapsMs) : null;

const allowedLatinTokens = new Set(["meetily", "m100"]);
const latinTokens = [...hypothesis.matchAll(/[\p{Script=Latin}][\p{Script=Latin}\p{N}.-]*/gu)].map((match) =>
  match[0].toLocaleLowerCase("en-US"),
);
const unexpectedLatinTokens = latinTokens.filter((token) => !allowedLatinTokens.has(token));
const latinRuns = [
  ...hypothesis.matchAll(
    /[\p{Script=Latin}][\p{Script=Latin}\p{N}.-]*(?:[\s,;:!?，。；：！？]+[\p{Script=Latin}][\p{Script=Latin}\p{N}.-]*)+/gu,
  ),
].map((match) => match[0]);
const forbiddenLeakage = ["qwen", "large-v3", "large v3", "turbo"].filter((term) =>
  hypothesis.toLocaleLowerCase("en-US").includes(term),
);
const traditionalOnly = [
  "會", "議", "錄", "轉", "寫", "與", "環", "參", "為", "項", "結", "論", "須",
  "綁", "動", "負", "責", "復", "報", "錯", "風", "險", "遲", "體", "輸", "後",
  "繼", "續", "請", "確", "術", "語", "認", "應", "編", "數", "決", "這", "裡",
].filter((character) => hypothesis.includes(character));

const normalizedSegments = (finalLive?.segments ?? []).map((segment) => normalize(segment.text ?? ""));
const duplicateSegments = normalizedSegments
  .map((text, index) => ({ index, text }))
  .filter(({ index, text }) => text && normalizedSegments.indexOf(text) !== index);
const timestampsMonotonic = (finalLive?.segments ?? []).every(
  (segment, index, segments) =>
    index === 0 ||
    (segment.audio_start_time >= segments[index - 1].audio_start_time &&
      segment.audio_end_time >= segments[index - 1].audio_end_time),
);

const coreChecks = {
  first_visible_within_8s: firstVisibleMs !== null && firstVisibleMs >= 0 && firstVisibleMs <= 8000,
  continuous_updates_within_10s: maxUpdateGapMs !== null && maxUpdateGapMs <= 10000,
  live_cer_at_most_15_percent: cer <= 0.15,
  no_cross_language_latin_run: latinRuns.length === 0,
  no_prompt_leakage: forbiddenLeakage.length === 0,
  no_replacement_character: !hypothesis.includes("�"),
  simplified_chinese_target: traditionalOnly.length === 0,
  no_exact_duplicate_segments: duplicateSegments.length === 0,
  timestamps_monotonic: timestampsMonotonic,
};
const entityChecks = {
  meetily_entity_exact: normalizedHypothesis.includes(normalize("Meetily")),
  m100_entity_exact: normalizedHypothesis.includes(normalize("M100")),
};
const liveCoreVerdict = Object.values(coreChecks).every(Boolean) ? "PASS" : "FAIL";
const strictEntityVerdict = Object.values(entityChecks).every(Boolean) ? "PASS" : "FAIL";

const result = {
  generatedAt: new Date().toISOString(),
  input: path.resolve(monitorPath),
  playbackStartedAt,
  monitorStartedAt: monitor.startedAt,
  reference,
  hypothesis,
  normalizedReferenceLength: normalizedReference.length,
  normalizedHypothesisLength: normalizedHypothesis.length,
  editDistance,
  cer,
  cerPercent: Number((cer * 100).toFixed(4)),
  firstVisibleMsAfterAudioStart: firstVisibleMs,
  maxUpdateGapMs,
  updateCount: nonempty.length,
  lastLiveSnapshotAt: finalLive?.at ?? null,
  finalLiveSegmentCount: finalLive?.segmentCount ?? 0,
  latinTokens,
  unexpectedLatinTokens,
  latinRuns,
  forbiddenLeakage,
  traditionalOnly,
  duplicateSegments,
  timestampsMonotonic,
  coreChecks,
  entityChecks,
  liveCoreVerdict,
  strictEntityVerdict,
  verdict:
    liveCoreVerdict === "PASS" && strictEntityVerdict === "PASS"
      ? "PASS"
      : liveCoreVerdict === "PASS"
        ? "PASS_WITH_ENTITY_MISS"
        : "FAIL",
};

fs.mkdirSync(path.dirname(outputPath), { recursive: true });
fs.writeFileSync(outputPath, `${JSON.stringify(result, null, 2)}\n`, "utf8");
console.log(JSON.stringify(result, null, 2));
