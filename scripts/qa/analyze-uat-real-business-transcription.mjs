import fs from "node:fs";
import path from "node:path";

const [monitorPath, clockPath, transcriptPath, metadataPath, outputPath] = process.argv.slice(2);
if (!monitorPath || !clockPath || !transcriptPath || !metadataPath || !outputPath) {
  throw new Error(
    "Usage: node analyze-uat-real-business-transcription.mjs <monitor.json> <clock.json> <transcripts.json> <metadata.json> <output.json>",
  );
}

const readJson = (filePath) => JSON.parse(fs.readFileSync(filePath, "utf8"));
const monitor = readJson(monitorPath);
const clock = readJson(clockPath);
const transcripts = readJson(transcriptPath);
const metadata = readJson(metadataPath);

const isAsciiWord = (character) => /[A-Za-z0-9_]/.test(character ?? "");
function countToken(input, token, caseSensitive = true) {
  const haystack = caseSensitive ? input : input.toLocaleLowerCase("en-US");
  const needle = caseSensitive ? token : token.toLocaleLowerCase("en-US");
  let count = 0;
  let cursor = 0;
  while (cursor <= haystack.length - needle.length) {
    const index = haystack.indexOf(needle, cursor);
    if (index < 0) break;
    const end = index + needle.length;
    if (!isAsciiWord(input[index - 1]) && !isAsciiWord(input[end])) count += 1;
    cursor = index + Math.max(1, needle.length);
  }
  return count;
}

const percentile = (values, fraction) => {
  if (values.length === 0) return null;
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.min(sorted.length - 1, Math.ceil(sorted.length * fraction) - 1)];
};

const positiveChanges = monitor.changes.filter((change) => change.segmentCount > 0);
const firstPositive = positiveChanges[0] ?? null;
const maxSegmentCount = Math.max(0, ...positiveChanges.map((change) => change.segmentCount));
const maxSnapshot = [...positiveChanges]
  .reverse()
  .find((change) => change.segmentCount === maxSegmentCount) ?? null;
const playbackStartedMs = Date.parse(clock.playbackStartedAt);
const firstVisibleDelayMs = firstPositive ? Date.parse(firstPositive.at) - playbackStartedMs : null;
const firstChinese = positiveChanges.find((change) => /[\u3400-\u9fff]/u.test(change.text)) ?? null;
const firstChineseDelayMs = firstChinese ? Date.parse(firstChinese.at) - playbackStartedMs : null;

let appendOnly = true;
let countRegressions = 0;
let countJumps = 0;
for (let index = 1; index < positiveChanges.length; index += 1) {
  const previous = positiveChanges[index - 1];
  const current = positiveChanges[index];
  if (current.segmentCount < previous.segmentCount) countRegressions += 1;
  if (current.segmentCount > previous.segmentCount + 1) countJumps += 1;
  const previousSegments = previous.segments ?? [];
  const currentSegments = current.segments ?? [];
  if (previousSegments.length > currentSegments.length) {
    appendOnly = false;
    continue;
  }
  for (let segmentIndex = 0; segmentIndex < previousSegments.length; segmentIndex += 1) {
    const before = previousSegments[segmentIndex];
    const after = currentSegments[segmentIndex];
    if (!after || before.id !== after.id || before.text !== after.text) {
      appendOnly = false;
      break;
    }
  }
}

const updateGapsMs = positiveChanges.slice(1).map((change, index) =>
  Date.parse(change.at) - Date.parse(positiveChanges[index].at),
);
const overTenSecondWindows = positiveChanges.slice(1).flatMap((change, index) => {
  const previous = positiveChanges[index];
  const gapMs = Date.parse(change.at) - Date.parse(previous.at);
  if (gapMs <= 10000) return [];
  const previousLast = previous.segments?.at(-1) ?? null;
  const currentLast = change.segments?.at(-1) ?? null;
  return [{
    gapMs,
    beforeAt: previous.at,
    afterAt: change.at,
    beforeSegmentCount: previous.segmentCount,
    afterSegmentCount: change.segmentCount,
    beforeLastText: previousLast?.text ?? null,
    afterLastText: currentLast?.text ?? null,
    sourceAudioSilenceGapSeconds: previousLast && currentLast
      ? currentLast.audio_start_time - previousLast.audio_end_time
      : null,
  }];
});
const liveSegments = maxSnapshot?.segments ?? [];
const liveText = liveSegments.map((segment) => segment.text).join("\n");
const liveDuplicateGroups = Object.entries(
  liveSegments.reduce((groups, segment) => {
    const text = segment.text.trim();
    if (text) groups[text] = (groups[text] ?? 0) + 1;
    return groups;
  }, {}),
).filter(([, count]) => count > 1);
const liveEnglishOnlySegments = liveSegments.filter((segment) =>
  /[A-Za-z]{4}/.test(segment.text) && !/[\u3400-\u9fff]/u.test(segment.text),
);

const finalSegments = transcripts.segments ?? [];
const finalText = finalSegments.map((segment) => segment.text).join("\n");
const finalDuplicateGroups = Object.entries(
  finalSegments.reduce((groups, segment) => {
    const text = segment.text.trim();
    if (text) groups[text] = (groups[text] ?? 0) + 1;
    return groups;
  }, {}),
).filter(([, count]) => count > 1);
const sequenceContinuous = finalSegments.every((segment, index) => segment.sequence_id === index);
const uniqueIds = new Set(finalSegments.map((segment) => segment.id)).size === finalSegments.length;
const timestampsMonotonic = finalSegments.every((segment, index) =>
  index === 0 || segment.audio_start_time >= finalSegments[index - 1].audio_end_time,
);
const durationConsistent = finalSegments.every((segment) =>
  Math.abs(segment.duration - (segment.audio_end_time - segment.audio_start_time)) < 0.001,
);

const entityCounts = (text) => ({
  YouTube: {
    canonical: countToken(text, "YouTube"),
    caseInsensitiveFamily: countToken(text, "YouTube", false),
    residualVariants: {
      Youtube: countToken(text, "Youtube"),
      youtube: countToken(text, "youtube"),
      YOUTUBE: countToken(text, "YOUTUBE"),
      U2B: countToken(text, "U2B", false),
    },
  },
  PWA: {
    canonical: countToken(text, "PWA"),
    caseInsensitiveFamily: countToken(text, "PWA", false),
    residualPW: countToken(text, "PW", false),
  },
  H5: countToken(text, "H5", false),
  Google: countToken(text, "Google", false),
  M100: countToken(text, "M100", false),
  people: {
    Amu: countToken(text, "Amu"),
    residualAmuAlias: (text.match(/阿木/gu) ?? []).length,
    伊犁: (text.match(/伊犁/gu) ?? []).length,
    residualYiliAlias: (text.match(/异利/gu) ?? []).length,
    卢真: (text.match(/卢真/gu) ?? []).length,
    residualLuzhenAlias: (text.match(/老卢/gu) ?? []).length,
  },
});

const liveEntities = entityCounts(liveText);
const finalEntities = entityCounts(finalText);
const residualTermVariants = Object.values(finalEntities.YouTube.residualVariants)
  .reduce((sum, count) => sum + count, 0) + finalEntities.PWA.residualPW;
const residualConfiguredAliases = finalEntities.people.residualAmuAlias
  + finalEntities.people.residualYiliAlias
  + finalEntities.people.residualLuzhenAlias;

const report = {
  analyzedAt: new Date().toISOString(),
  inputs: { monitorPath, clockPath, transcriptPath, metadataPath },
  meeting: {
    name: clock.meetingName,
    folder: clock.meetingFolder,
    templateId: metadata.summary_template?.template_id ?? null,
    templateVersion: metadata.summary_template?.template_version ?? null,
    recordingContextId: metadata.meeting_context?.recording_context_id ?? null,
    currentContextId: metadata.meeting_context?.current_context_id ?? null,
    durationSeconds: metadata.duration_seconds ?? null,
    retranscribedAt: metadata.retranscribed_at ?? null,
  },
  live: {
    positiveChangeCount: positiveChanges.length,
    maxSegmentCount,
    firstVisibleDelayMs,
    firstChineseDelayMs,
    appendOnly,
    countRegressions,
    countJumps,
    gapMs: {
      average: updateGapsMs.length
        ? updateGapsMs.reduce((sum, value) => sum + value, 0) / updateGapsMs.length
        : null,
      p50: percentile(updateGapsMs, 0.5),
      p95: percentile(updateGapsMs, 0.95),
      maximum: updateGapsMs.length ? Math.max(...updateGapsMs) : null,
      overTenSeconds: updateGapsMs.filter((value) => value > 10000),
      overTenSecondWindows,
    },
    duplicateGroups: liveDuplicateGroups,
    englishOnlySegments: liveEnglishOnlySegments.map((segment) => ({
      sequenceId: segment.sequence_id,
      text: segment.text,
    })),
    entities: liveEntities,
  },
  final: {
    segmentCount: finalSegments.length,
    declaredSegmentCount: transcripts.total_segments,
    sequenceContinuous,
    uniqueIds,
    timestampsMonotonic,
    durationConsistent,
    duplicateGroups: finalDuplicateGroups,
    replacementCharacterCount: (finalText.match(/�/gu) ?? []).length,
    entities: finalEntities,
  },
  focusedRetestVerdict: {
    recordingAndBubblesCompleted: maxSegmentCount > 0 && metadata.duration_seconds > 0,
    firstVisibleWithinEightSeconds: firstVisibleDelayMs !== null && firstVisibleDelayMs <= 8000,
    noLiveGapOverTenSeconds: updateGapsMs.every((value) => value <= 10000),
    noUnexplainedLiveGapOverTenSeconds: overTenSecondWindows.every((window) =>
      window.sourceAudioSilenceGapSeconds !== null
        && window.sourceAudioSilenceGapSeconds >= 2,
    ),
    liveWasAppendOnly: appendOnly && countRegressions === 0 && countJumps === 0,
    noLiveEnglishOnlyGarbageSegment: liveEnglishOnlySegments.length === 0,
    finalStructureValid: finalSegments.length === transcripts.total_segments
      && sequenceContinuous && uniqueIds && timestampsMonotonic && durationConsistent
      && finalDuplicateGroups.length === 0,
    finalCanonicalYouTubeOnly: finalEntities.YouTube.canonical > 0
      && finalEntities.YouTube.canonical === finalEntities.YouTube.caseInsensitiveFamily
      && Object.values(finalEntities.YouTube.residualVariants).every((count) => count === 0),
    finalCanonicalPwaOnly: finalEntities.PWA.canonical > 0
      && finalEntities.PWA.canonical === finalEntities.PWA.caseInsensitiveFamily
      && finalEntities.PWA.residualPW === 0,
    configuredPersonAliasesNormalized: finalEntities.people.Amu > 0
      && finalEntities.people.卢真 > 0
      && residualConfiguredAliases === 0,
  },
};

fs.mkdirSync(path.dirname(outputPath), { recursive: true });
fs.writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
console.log(JSON.stringify(report, null, 2));
