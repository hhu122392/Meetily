import fs from "node:fs";

const restorePath = process.argv[2];
const persistedPath = process.argv[3];
if (!restorePath || !persistedPath) {
  throw new Error("Usage: node audit-t07-restore-evidence.mjs <restore.json> <persisted-state.json>");
}
const restore = JSON.parse(fs.readFileSync(restorePath, "utf8"));
const persisted = JSON.parse(fs.readFileSync(persistedPath, "utf8"));
const restoredMarkdown = persisted.summary?.data?.markdown ?? "";
const restoredGenerationId = persisted.summary?.data?.template_snapshot?.generationId ?? null;
const sourceGenerationId = restore.manualAfter?.[0]?.sourceGenerationId ?? null;
const verdict = {
  priorGeneratedSummaryHadNoMarker: restore.bodyBeforeContainsMarker === false,
  restoreSuccessSurfaced: restore.toastSeen === true,
  restoredMarkerVisibleImmediately: restore.bodyAfterContainsMarker === true,
  restoredMarkerPersistedAfterReload: persisted.bodyHasSavedMarker === true
    && restoredMarkdown.includes("【摘要人工校正】"),
  generationHistoryUnchanged: restore.historyAfter.length === restore.historyBefore.length,
  manualHistoryUnchanged: restore.manualAfter.length === restore.manualBefore.length,
  manualRevisionNowCurrent: restore.manualAfter.some((revision) => revision.isCurrent),
  sourceGenerationNowCurrent: restore.historyAfter.some((item) => (
    item.generationId === sourceGenerationId && item.isCurrentSummary
  )),
  restoredSnapshotMatchesManualSource: restoredGenerationId === sourceGenerationId,
};
console.log(JSON.stringify({
  auditedAt: new Date().toISOString(),
  restoreEvidence: restorePath,
  persistedEvidence: persistedPath,
  correctionReason: "原始恢复脚本误用不含摘要正文的 api_get_meeting 判断内容变化；本复核使用恢复前后页面标记、api_get_summary 刷新后正文及当前版本指针。",
  sourceGenerationId,
  restoredGenerationId,
  verdict,
  pass: Object.values(verdict).every(Boolean),
}, null, 2));
