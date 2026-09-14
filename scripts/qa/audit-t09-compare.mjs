import fs from "node:fs";

const [beforePath, afterRestartPath, finalPath] = process.argv.slice(2);
if (!beforePath || !afterRestartPath || !finalPath) {
  throw new Error("Usage: node audit-t09-compare.mjs <before.json> <after-restart.json> <final.json>");
}
const before = JSON.parse(fs.readFileSync(beforePath, "utf8"));
const after = JSON.parse(fs.readFileSync(afterRestartPath, "utf8"));
const final = JSON.parse(fs.readFileSync(finalPath, "utf8"));

const stable = (meeting) => ({
  id: meeting.database.id,
  title: meeting.database.title,
  createdAt: meeting.database.created_at,
  updatedAt: meeting.database.updated_at,
  folderPath: meeting.database.folder_path,
  metadataSha256: meeting.metadata_sha256,
  transcriptsFileSha256: meeting.transcripts_file_sha256,
  transcriptSha256: meeting.transcript_sha256,
  transcriptCount: meeting.transcript_count,
  media: meeting.media,
  summaryMarkdownSha256: meeting.summary_markdown_sha256,
  history: meeting.history,
  manualRevisions: meeting.manual_revisions,
});

const beforeLive = stable(before.live);
const afterLive = stable(after.live);
const beforeImported = stable(before.imported);
const afterImported = stable(after.imported);
const verdict = {
  beforeAuditPassed: Object.values(before.verdict).every(Boolean),
  restartAuditPassed: Object.values(after.verdict).every(Boolean),
  finalAuditPassed: Object.values(final.verdict).every(Boolean),
  liveExactlyStableAcrossRestart: JSON.stringify(beforeLive) === JSON.stringify(afterLive),
  importedExactlyStableAcrossRestart: JSON.stringify(beforeImported) === JSON.stringify(afterImported),
  liveIdentityAudioAndTranscriptStableAfterOperation:
    final.live.database.id === after.live.database.id
    && final.live.database.folder_path === after.live.database.folder_path
    && JSON.stringify(final.live.media) === JSON.stringify(after.live.media)
    && final.live.transcript_sha256 === after.live.transcript_sha256,
  importedStillExactlyStableAfterOperation: JSON.stringify(stable(final.imported)) === JSON.stringify(afterImported),
  explicitSaveCreatedExactlyOneManualRevision:
    final.live.manual_revision_count === after.live.manual_revision_count + 1,
  explicitSaveDidNotCreateGeneration:
    final.live.history_count === after.live.history_count,
  bothSummaryMarkersPersisted:
    final.live.summary_saved_marker_count === 1 && final.live.summary_restart_marker_count === 1,
  persistedTargetTasksRemainIdle:
    final.global_running_summary_count === 0
    && final.live.metadata.status === 'completed'
    && final.imported.metadata.status === 'completed',
};
console.log(JSON.stringify({
  auditedAt: new Date().toISOString(),
  beforePath,
  afterRestartPath,
  finalPath,
  counts: {
    liveHistory: [before.live.history_count, after.live.history_count, final.live.history_count],
    liveManualRevisions: [before.live.manual_revision_count, after.live.manual_revision_count, final.live.manual_revision_count],
    importedHistory: [before.imported.history_count, after.imported.history_count, final.imported.history_count],
  },
  verdict,
  pass: Object.values(verdict).every(Boolean),
}, null, 2));
