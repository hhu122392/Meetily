import fs from "node:fs";

const evidencePath = process.argv[2];
if (!evidencePath) throw new Error("Usage: node audit-t08-failure-evidence.mjs <evidence.json>");
const evidence = JSON.parse(fs.readFileSync(evidencePath, "utf8"));
const markerVisibleAfterFailure = evidence.bodyTextTail.includes("【摘要人工校正】");
const verdict = {
  ...evidence.verdict,
  savedEditPreserved: evidence.summaryUnchanged && markerVisibleAfterFailure,
};
console.log(JSON.stringify({
  auditedAt: new Date().toISOString(),
  sourceEvidence: evidencePath,
  correctionReason: "原始审计器只在序列化的摘要 DTO 外层搜索标记；实际标记位于渲染内容中。摘要对象前后全量相等，且失败后的页面正文仍显示人工校正标记。",
  unchangedSummaryObject: evidence.summaryUnchanged,
  markerVisibleAfterFailure,
  historyCountBefore: evidence.historyCountBefore,
  historyCountAfter: evidence.historyCountAfter,
  manualRevisionCountBefore: evidence.manualRevisionCountBefore,
  manualRevisionCountAfter: evidence.manualRevisionCountAfter,
  maxRelevantToastCount: evidence.maxRelevantToastCount,
  maxRelevantAlertCount: evidence.maxRelevantAlertCount,
  verdict,
  pass: Object.values(verdict).every(Boolean),
}, null, 2));
