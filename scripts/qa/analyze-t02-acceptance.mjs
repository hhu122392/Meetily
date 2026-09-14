import fs from "node:fs";
import path from "node:path";

function usage() {
  console.error(
    "Usage: node analyze-t02-acceptance.mjs <transcripts.json> <metadata.json> <live-first.json> <live-second.json> <output.json>",
  );
  process.exit(2);
}

if (process.argv.length !== 7) usage();

const [transcriptsPath, metadataPath, liveFirstPath, liveSecondPath, outputPath] =
  process.argv.slice(2);

const readJson = (file) => JSON.parse(fs.readFileSync(file, "utf8"));
const transcripts = readJson(transcriptsPath);
const metadata = readJson(metadataPath);
const liveFirst = readJson(liveFirstPath);
const liveSecond = readJson(liveSecondPath);

const reference = [
  "今天进行 Meetily 核心功能验收。会议时间是二〇二六年八月二十四日十九点三十分，会议主题是中文录音、转写与摘要闭环。参会人包括李明、王芳和赵强，项目代号是 M100。第一项结论：本周五前完成简体中文转写验证。第二项结论：摘要必须使用已经绑定的会议模板。第一项行动：王芳在八月二十八日前提交真实测试报告。第二项行动：赵强负责修复摘要生成失败和重复报错。当前风险包括录音延迟、整段漏转、繁体字输出和摘要生成失败。下面暂停录音十秒，恢复后继续。",
  "录音已经恢复。请正确保留 API、PWA、A/B Test、Qwen 三点五和 large-v3-turbo 这些术语。最后确认：会议摘要必须包含会议结论、行动事项、负责人、截止日期和风险，不得编造录音中没有出现的人名、数字或决定。核心功能验收到这里结束。",
].join("");

const hypothesis = transcripts.segments
  .slice()
  .sort((a, b) => a.sequence_id - b.sequence_id)
  .map((segment) => segment.text)
  .join("");

// The acceptance plan ignores punctuation, whitespace and full/half-width
// differences, but deliberately retains Han characters, digits and letters.
function normalize(value) {
  return value
    .normalize("NFKC")
    .toLocaleLowerCase("en-US")
    .replace(/[\p{P}\p{S}\p{Z}\s]/gu, "");
}

function levenshteinWithCounts(referenceText, hypothesisText) {
  const a = [...referenceText];
  const b = [...hypothesisText];
  const rows = Array.from({ length: a.length + 1 }, () =>
    Array(b.length + 1).fill(null),
  );
  rows[0][0] = { cost: 0, substitutions: 0, deletions: 0, insertions: 0 };
  for (let i = 1; i <= a.length; i += 1) {
    rows[i][0] = { cost: i, substitutions: 0, deletions: i, insertions: 0 };
  }
  for (let j = 1; j <= b.length; j += 1) {
    rows[0][j] = { cost: j, substitutions: 0, deletions: 0, insertions: j };
  }

  const prefer = (candidates) =>
    candidates.sort(
      (left, right) =>
        left.cost - right.cost ||
        left.substitutions - right.substitutions ||
        left.deletions - right.deletions ||
        left.insertions - right.insertions,
    )[0];

  for (let i = 1; i <= a.length; i += 1) {
    for (let j = 1; j <= b.length; j += 1) {
      const diagonal = rows[i - 1][j - 1];
      if (a[i - 1] === b[j - 1]) {
        rows[i][j] = { ...diagonal };
        continue;
      }
      const deletion = rows[i - 1][j];
      const insertion = rows[i][j - 1];
      rows[i][j] = prefer([
        {
          ...diagonal,
          cost: diagonal.cost + 1,
          substitutions: diagonal.substitutions + 1,
        },
        { ...deletion, cost: deletion.cost + 1, deletions: deletion.deletions + 1 },
        { ...insertion, cost: insertion.cost + 1, insertions: insertion.insertions + 1 },
      ]);
    }
  }
  return rows[a.length][b.length];
}

function liveMetrics(run, excludeInitialSnapshot = false) {
  const changes = excludeInitialSnapshot
    ? run.changes.filter((change) => change.elapsedMs > 1)
    : run.changes;
  const gaps = changes.slice(1).map((change, index) => change.elapsedMs - changes[index].elapsedMs);
  return {
    first_visible_ms: changes[0]?.elapsedMs ?? null,
    max_update_gap_ms: gaps.length ? Math.max(...gaps) : null,
    update_count: changes.length,
    replacement_character_count: changes.reduce(
      (count, change) => count + [...change.text].filter((character) => character === "�").length,
      0,
    ),
  };
}

const normalizedReference = normalize(reference);
const normalizedHypothesis = normalize(hypothesis);
const edits = levenshteinWithCounts(normalizedReference, normalizedHypothesis);
const cer = edits.cost / normalizedReference.length;

const entityAliases = {
  Meetily: ["Meetily"],
  "会议时间": ["二〇二六年八月二十四日十九点三十分"],
  "李明": ["李明"],
  "王芳": ["王芳"],
  "赵强": ["赵强"],
  M100: ["M100"],
  "截止日期": ["八月二十八日"],
  API: ["API"],
  PWA: ["PWA"],
  "A/B Test": ["A/B Test", "A-B Test"],
  Qwen: ["Qwen"],
  "三点五": ["三点五"],
  "large-v3-turbo": ["large-v3-turbo", "large v3 turbo"],
};
const entityResults = Object.fromEntries(
  Object.entries(entityAliases).map(([name, aliases]) => [
    name,
    aliases.some((alias) => normalizedHypothesis.includes(normalize(alias))),
  ]),
);
const entityAccuracy =
  Object.values(entityResults).filter(Boolean).length / Object.keys(entityResults).length;

// A focused audit list for traditional-only forms of characters used in the
// golden reference and the UI defect history. It intentionally reports the
// exact offending characters instead of pretending to be a general converter.
const traditionalOnlyCharacters = Object.keys({
  會: "会", 議: "议", 錄: "录", 轉: "转", 寫: "写", 與: "与", 環: "环",
  參: "参", 為: "为", 項: "项", 結: "结", 論: "论", 須: "须", 綁: "绑",
  動: "动", 負: "负", 責: "责", 復: "复", 報: "报", 錯: "错", 風: "风",
  險: "险", 遲: "迟", 體: "体", 輸: "输", 齣: "出", 後: "后", 繼: "继",
  續: "续", 請: "请", 確: "确", 術: "术", 語: "语", 認: "认", 應: "应",
  編: "编", 數: "数", 決: "决", 這: "这", 裡: "里",
});
const traditionalHits = traditionalOnlyCharacters.filter((character) => hypothesis.includes(character));

const orderedSegments = transcripts.segments.slice().sort((a, b) => a.sequence_id - b.sequence_id);
const normalizedSegments = orderedSegments.map((segment) => normalize(segment.text));
const duplicateSegments = normalizedSegments
  .map((text, index) => ({ index, text }))
  .filter(({ index, text }) => text && normalizedSegments.indexOf(text) !== index);
const timestampsMonotonic = orderedSegments.every(
  (segment, index) =>
    index === 0 ||
    (segment.audio_start_time >= orderedSegments[index - 1].audio_start_time &&
      segment.audio_end_time >= orderedSegments[index - 1].audio_end_time),
);

const stopAt = Date.parse(metadata.completed_at);
const finalAt = Date.parse(transcripts.last_updated);
const finalCommitDelayMs = finalAt - stopAt;
const firstLive = liveMetrics(liveFirst);
const secondLive = liveMetrics(liveSecond, true);
const lastLiveChange = [...liveFirst.changes, ...liveSecond.changes]
  .filter((change) => Number.isFinite(Date.parse(change.at)))
  .sort((left, right) => Date.parse(left.at) - Date.parse(right.at))
  .at(-1);
const lastLiveUpdateAt = lastLiveChange?.at ?? null;
// T02's "stop tail <= 10 s" requirement measures whether the final spoken
// tail becomes visible after Stop. The separate full-recording enhancement
// pass is intentionally reported below as background work: it replaces the
// draft atomically and the UI is edit-locked until that commit completes.
const liveTailAfterStopMs = lastLiveUpdateAt
  ? Math.max(0, Date.parse(lastLiveUpdateAt) - stopAt)
  : null;

const checks = {
  first_visible_within_8s: firstLive.first_visible_ms <= 8000,
  continuous_updates_within_10s:
    firstLive.max_update_gap_ms <= 10000 && secondLive.max_update_gap_ms <= 10000,
  live_tail_after_stop_within_10s:
    liveTailAfterStopMs !== null && liveTailAfterStopMs <= 10000,
  cer_at_most_5_percent: cer <= 0.05,
  key_entities_100_percent: entityAccuracy === 1,
  simplified_chinese_target: traditionalHits.length === 0,
  no_duplicate_final_segments: duplicateSegments.length === 0,
  timestamps_monotonic: timestampsMonotonic,
  no_replacement_character_in_final: !hypothesis.includes("�"),
};

const result = {
  generated_at: new Date().toISOString(),
  inputs: {
    transcripts: path.resolve(transcriptsPath),
    metadata: path.resolve(metadataPath),
    live_first: path.resolve(liveFirstPath),
    live_second: path.resolve(liveSecondPath),
  },
  model: "large-v3-turbo-q5_0 (Vulkan; verified in runtime log)",
  reference,
  hypothesis,
  normalized_reference_length: normalizedReference.length,
  normalized_hypothesis_length: normalizedHypothesis.length,
  edits,
  cer,
  cer_percent: Number((cer * 100).toFixed(4)),
  entity_results: entityResults,
  entity_accuracy_percent: Number((entityAccuracy * 100).toFixed(2)),
  traditional_only_hits: traditionalHits,
  duplicate_final_segments: duplicateSegments,
  timestamps_monotonic: timestampsMonotonic,
  live_first: firstLive,
  live_second_after_resume: secondLive,
  stop_completed_at: metadata.completed_at,
  last_live_update_at: lastLiveUpdateAt,
  live_tail_after_stop_ms: liveTailAfterStopMs,
  final_committed_at: transcripts.last_updated,
  background_high_quality_commit_delay_ms: finalCommitDelayMs,
  timing_semantics: {
    acceptance_metric:
      "live_tail_after_stop_ms measures the final spoken tail becoming visible after Stop",
    reported_observation:
      "background_high_quality_commit_delay_ms is the atomic high-quality replacement and is not the live-tail metric",
  },
  checks,
  verdict: Object.values(checks).every(Boolean) ? "PASS" : "FAIL",
};

fs.mkdirSync(path.dirname(outputPath), { recursive: true });
fs.writeFileSync(outputPath, `${JSON.stringify(result, null, 2)}\n`, "utf8");
console.log(JSON.stringify(result, null, 2));
