import fs from "node:fs";
import path from "node:path";

const [transcriptPath, outputPath] = process.argv.slice(2);
if (!transcriptPath || !outputPath) {
  throw new Error(
    "Usage: node analyze-final-transcript-acceptance.mjs <transcripts.json> <output.json>",
  );
}

const reference =
  "今天进行 Meetily 核心功能验收。会议时间是二〇二六年八月二十四日十九点三十分，会议主题是中文录音、转写与摘要闭环。参会人包括李明、王芳和赵强，项目代号是 M100。第一项结论：本周五前完成简体中文转写验证。第二项结论：摘要必须使用已经绑定的会议模板。第一项行动：王芳在八月二十八日前提交真实测试报告。第二项行动：赵强负责修复摘要生成失败和重复报错。当前风险包括录音延迟、整段漏转、繁体字输出和摘要生成失败。下面暂停录音十秒，恢复后继续。";

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

const document = JSON.parse(fs.readFileSync(transcriptPath, "utf8"));
const segments = [...(document.segments ?? [])].sort(
  (left, right) => (left.sequence_id ?? 0) - (right.sequence_id ?? 0),
);
const hypothesis = segments.map((segment) => String(segment.text ?? "").trim()).join("");
const normalizedReference = normalize(reference);
const normalizedHypothesis = normalize(hypothesis);
const editDistance = levenshtein(normalizedReference, normalizedHypothesis);
const cer = editDistance / normalizedReference.length;

const forbiddenLeakage = ["qwen", "large-v3", "large v3", "turbo"].filter((term) =>
  hypothesis.toLocaleLowerCase("en-US").includes(term),
);
const traditionalOnly = [
  "會", "議", "錄", "轉", "寫", "與", "環", "參", "為", "項", "結", "論", "須",
  "綁", "動", "負", "責", "復", "報", "錯", "風", "險", "遲", "體", "輸", "後",
  "繼", "續", "請", "確", "術", "語", "認", "應", "編", "數", "決", "這", "裡",
].filter((character) => hypothesis.includes(character));
const normalizedSegments = segments.map((segment) => normalize(segment.text ?? ""));
const duplicateSegments = normalizedSegments
  .map((text, index) => ({ index, text }))
  .filter(({ index, text }) => text && normalizedSegments.indexOf(text) !== index);
const timestampsMonotonic = segments.every(
  (segment, index) =>
    index === 0 ||
    (segment.sequence_id >= segments[index - 1].sequence_id &&
      segment.audio_start_time >= segments[index - 1].audio_start_time &&
      segment.audio_end_time >= segments[index - 1].audio_end_time),
);

const coreChecks = {
  final_cer_at_most_5_percent: cer <= 0.05,
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
const finalCoreVerdict = Object.values(coreChecks).every(Boolean) ? "PASS" : "FAIL";
const strictEntityVerdict = Object.values(entityChecks).every(Boolean) ? "PASS" : "FAIL";

const result = {
  generatedAt: new Date().toISOString(),
  input: path.resolve(transcriptPath),
  reference,
  hypothesis,
  segmentCount: segments.length,
  normalizedReferenceLength: normalizedReference.length,
  normalizedHypothesisLength: normalizedHypothesis.length,
  editDistance,
  cer,
  cerPercent: Number((cer * 100).toFixed(4)),
  forbiddenLeakage,
  traditionalOnly,
  duplicateSegments,
  timestampsMonotonic,
  coreChecks,
  entityChecks,
  finalCoreVerdict,
  strictEntityVerdict,
  verdict:
    finalCoreVerdict === "PASS" && strictEntityVerdict === "PASS"
      ? "PASS"
      : finalCoreVerdict === "PASS"
        ? "PASS_WITH_ENTITY_MISS"
        : "FAIL",
};

fs.mkdirSync(path.dirname(outputPath), { recursive: true });
fs.writeFileSync(outputPath, `${JSON.stringify(result, null, 2)}\n`, "utf8");
console.log(JSON.stringify(result, null, 2));
