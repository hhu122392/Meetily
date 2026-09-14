import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const meetingId = process.argv[2];
const mode = process.argv[3] ?? "route";
const baselinePath = process.argv[4];
const outputPath = process.argv[5];
const screenshotPath = process.argv[6];
if (!meetingId || !baselinePath || !outputPath || !["route", "current"].includes(mode)) {
  throw new Error(
    "Usage: node cdp-uat06-revalidation-reopen.mjs <meeting-id> <route|current> <save-baseline.json> <output.json> [screenshot.png]",
  );
}
if (fs.existsSync(outputPath)) {
  throw new Error(`Refusing to overwrite existing evidence: ${outputPath}`);
}
if (screenshotPath && fs.existsSync(screenshotPath)) {
  throw new Error(`Refusing to overwrite existing screenshot: ${screenshotPath}`);
}

const baselineBytes = fs.readFileSync(baselinePath);
const baselineFileSha256 = crypto
  .createHash("sha256")
  .update(baselineBytes)
  .digest("hex")
  .toLowerCase();
const baseline = JSON.parse(baselineBytes.toString("utf8"));
if (baseline?.script !== "cdp-uat06-revalidation-save" || baseline?.schemaVersion !== 2) {
  throw new Error("Save baseline has an unsupported script or schema version");
}
if (baseline.meetingId !== meetingId) {
  throw new Error("Save baseline belongs to a different meeting");
}
const baselineRunnerSaveExecuted =
  baseline?.save?.runnerSaveExecuted === true ||
  (baseline?.save?.runnerSaveExecuted === undefined &&
    baseline?.save?.saveExecuted === true);
if (!baselineRunnerSaveExecuted || baseline?.verdict?.overallPass !== true) {
  throw new Error("Save baseline did not complete its one-time save and post-save checks");
}
const expected = baseline.after;
const addedRevisionId = baseline?.save?.addedRevisionId ?? "";
const expectedSavePayloadSha256 = baseline?.payloadEvidence?.savePayloadSha256 ?? "";
const expectedNativeFactValidationSha256 =
  baseline?.save?.responseFactValidationSha256 ?? "";
if (
  !expected?.summary?.markdownSha256 ||
  !expected?.transcripts?.projectionSha256 ||
  !/^[A-Za-z0-9][A-Za-z0-9._:-]{2,255}$/.test(addedRevisionId) ||
  !/^[a-f0-9]{64}$/.test(expectedSavePayloadSha256) ||
  !/^[a-f0-9]{64}$/.test(expectedNativeFactValidationSha256)
) {
  throw new Error("Save baseline is missing required post-save fingerprints");
}
const expectedRevision = expected.manualRevisions?.entries?.find(
  (entry) => entry.revisionId === addedRevisionId,
);
if (!expectedRevision) {
  throw new Error("Save baseline does not contain its dynamically added revision");
}
if (expected.manualRevisions?.projectionSchemaVersion !== 2) {
  throw new Error("Save baseline has an unsupported manual revision projection schema");
}

const pidBefore = Number(process.env.UAT_PID_BEFORE ?? 0);
const pidAfter = Number(process.env.UAT_PID_AFTER ?? 0);
const executablePath = process.env.UAT_EXE_PATH ?? "";
const expectedReleaseSha256 = (process.env.UAT_EXE_SHA256 ?? "").toUpperCase();
if (mode === "current") {
  if (!Number.isSafeInteger(pidBefore) || pidBefore <= 0) {
    throw new Error("current mode requires a positive UAT_PID_BEFORE");
  }
  if (!Number.isSafeInteger(pidAfter) || pidAfter <= 0) {
    throw new Error("current mode requires a positive UAT_PID_AFTER");
  }
  if (!executablePath || !fs.existsSync(executablePath)) {
    throw new Error("current mode requires an existing UAT_EXE_PATH");
  }
  if (!/^[A-F0-9]{64}$/.test(expectedReleaseSha256)) {
    throw new Error("current mode requires a 64-character UAT_EXE_SHA256");
  }
}

const allowedReadOnlyCommands = [
  "api_get_meeting",
  "api_get_summary",
  "api_list_summary_generation_history",
  "api_list_manual_summary_revisions",
  "get_recording_state",
  "is_retranscription_in_progress_command",
];
const destination = `http://tauri.localhost/meeting-details?id=${encodeURIComponent(meetingId)}`;

async function runReopenAudit(config) {
  const invoke = window.__TAURI_INTERNALS__?.invoke;
  const commandTrace = [];
  const summaryEditorCandidateSelectors = [
    ".bn-container",
    '.bn-container .bn-editor[contenteditable="true"]',
    '.bn-container .bn-editor [contenteditable="true"]',
    '.bn-container [contenteditable="true"].bn-editor',
    '.bn-container [contenteditable="true"]',
  ];
  if (typeof invoke !== "function") {
    return {
      schemaVersion: 2,
      script: "cdp-uat06-revalidation-reopen",
      capturedAt: new Date().toISOString(),
      meetingId: config.meetingId,
      mode: config.mode,
      commitState: "not_attempted",
      error: { message: "Tauri invoke bridge is unavailable" },
      commandTrace,
      verdict: { overallPass: false },
    };
  }

  const allowedReadOnlyCommands = new Set(config.allowedReadOnlyCommands);
  const sortDeep = (value) => {
    if (Array.isArray(value)) return value.map(sortDeep);
    if (!value || typeof value !== "object") return value;
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((key) => [key, sortDeep(value[key])]),
    );
  };
  const stableStringify = (value) => JSON.stringify(sortDeep(value));
  const sha256Text = async (value) => {
    if (!crypto?.subtle) throw new Error("Web Crypto SHA-256 is unavailable");
    const digest = await crypto.subtle.digest(
      "SHA-256",
      new TextEncoder().encode(String(value)),
    );
    return [...new Uint8Array(digest)]
      .map((byte) => byte.toString(16).padStart(2, "0"))
      .join("");
  };
  const normalizeVisibleText = (value) => String(value ?? "").replace(/\s+/g, " ").trim();
  const canonicalizeProbeText = (value) =>
    normalizeVisibleText(value).replace(
      /\s+([,.;:!?，。；：！？、）】》」』])/g,
      "$1",
    );
  const probeMatchesEditorText = (editorText, probe) => {
    const canonicalProbe = canonicalizeProbeText(probe);
    return (
      canonicalProbe.length > 0 &&
      canonicalizeProbeText(editorText).includes(canonicalProbe)
    );
  };
  const pickMarkdownProbe = (markdown) => {
    const candidates = String(markdown ?? "")
      .split(/\r?\n/)
      .map((original) => ({
        original,
        cleaned: normalizeVisibleText(
          original
            .replace(/^\s{0,3}(?:#{1,6}|[-*+] |\d+[.)] )\s*/, "")
            .replace(/[*_>#|]/g, " ")
            .replaceAll("`", " "),
        ),
      }))
      .filter((item) => item.cleaned.length >= 12);
    const prose = candidates.filter(
      (item) => !item.original.includes("|") && !/^\s*[-: ]+\s*$/.test(item.original),
    );
    const selected = (prose.length > 0 ? prose : candidates).sort(
      (left, right) => right.cleaned.length - left.cleaned.length,
    )[0];
    return selected?.cleaned.slice(0, 80) ?? null;
  };
  const isVisible = (element) => {
    if (!element) return false;
    const rect = element.getBoundingClientRect();
    const style = getComputedStyle(element);
    return (
      rect.width > 0 &&
      rect.height > 0 &&
      style.visibility !== "hidden" &&
      style.display !== "none"
    );
  };
  const findSummaryEditor = (probe) => {
    const containers = [...document.querySelectorAll(".bn-container")].filter(isVisible);
    const rawCandidates = containers
      .map((container) => {
        const editor =
          (container.matches('.bn-editor[contenteditable="true"]') ? container : null) ??
          container.querySelector('.bn-editor[contenteditable="true"]') ??
          container.querySelector('.bn-editor [contenteditable="true"]') ??
          container.querySelector('[contenteditable="true"].bn-editor') ??
          container.querySelector('[contenteditable="true"]');
        return editor && isVisible(editor)
          ? {
              container,
              editor,
              rawText: normalizeVisibleText(editor.innerText),
              canonicalText: canonicalizeProbeText(editor.innerText),
            }
          : null;
      })
      .filter(Boolean);
    const candidates = [...new Map(
      rawCandidates.map((candidate) => [candidate.editor, candidate]),
    ).values()];
    const rawProbe = String(probe ?? "");
    const normalizedProbe = normalizeVisibleText(rawProbe);
    const canonicalProbe = canonicalizeProbeText(rawProbe);
    // Never accept an editor merely because it is the only candidate. A
    // non-empty canonical probe must still be present in its canonical text.
    const matches = canonicalProbe
      ? candidates.filter((candidate) =>
          probeMatchesEditorText(candidate.rawText, rawProbe),
        )
      : [];
    return {
      containersFound: containers.length,
      editorsFound: candidates.length,
      probeMatches: matches.length,
      rawProbe,
      normalizedProbe,
      canonicalProbe,
      candidateTextEvidence: candidates.map((candidate) => ({
        rawTextTail: candidate.rawText.slice(-500),
        canonicalTextTail: candidate.canonicalText.slice(-500),
      })),
      selected: matches.length === 1 ? matches[0] : null,
    };
  };
  const waitForSummaryEditor = async (probe, timeoutMs = 15000) => {
    const started = performance.now();
    let state = findSummaryEditor(probe);
    while (performance.now() - started < timeoutMs) {
      if (state.selected) return state;
      await new Promise((resolve) => setTimeout(resolve, 100));
      state = findSummaryEditor(probe);
    }
    throw new Error(
      `Summary BlockNote editor/probe was not uniquely found: ${JSON.stringify({
        containersFound: state.containersFound,
        editorsFound: state.editorsFound,
        probeMatches: state.probeMatches,
        rawProbe: state.rawProbe,
        canonicalProbe: state.canonicalProbe,
        candidateTextEvidence: state.candidateTextEvidence,
      })}`,
    );
  };
  const factValidationConsistent = (validation) =>
    Boolean(
      validation &&
        ["passed", "needs_review"].includes(validation.status) &&
        Array.isArray(validation.warnings) &&
        validation.warningCount === validation.warnings.length,
    );
  const factValidationProjection = async (validation) => ({
    value: validation ?? null,
    sha256: await sha256Text(stableStringify(validation ?? null)),
    status: validation?.status ?? null,
    warningCount: validation?.warningCount ?? null,
    warningCodes: (validation?.warnings ?? [])
      .map((warning) => warning?.code ?? null)
      .filter(Boolean)
      .sort(),
    aliasesNormalized: validation?.aliasesNormalized ?? null,
    meetingContextId: validation?.meetingContextId ?? null,
    meetingContextSha256: validation?.meetingContextSha256 ?? null,
    summaryContextSha256: validation?.summaryContextSha256 ?? null,
  });
  const transcriptProjection = (meeting) =>
    (meeting?.transcripts ?? []).map((segment) => ({
      id: segment.id ?? null,
      text: segment.text ?? "",
      timestamp: segment.timestamp ?? null,
      audio_start_time: segment.audio_start_time ?? null,
      audio_end_time: segment.audio_end_time ?? null,
      duration: segment.duration ?? null,
    }));
  const manualRevisionProjectionSchemaVersion = 2;
  const manualRevisionProjection = async (revisions) =>
    Promise.all(
      revisions.map(async (revision) => {
        const storedFactValidation = revision.summary?.factValidation ?? null;
        const storedTemplateSnapshot =
          revision.summary?.template_snapshot ?? null;
        const storedFact = await factValidationProjection(storedFactValidation);
        return {
          revisionId: revision.revisionId ?? null,
          createdAt: revision.createdAt ?? null,
          sourceGenerationId: revision.sourceGenerationId ?? null,
          isCurrent: revision.isCurrent === true,
          markdownSha256: await sha256Text(revision.markdown ?? ""),
          summarySha256: await sha256Text(stableStringify(revision.summary ?? null)),
          storedTemplateSnapshotSha256: await sha256Text(
            stableStringify(storedTemplateSnapshot),
          ),
          storedTemplateSnapshot: storedTemplateSnapshot
            ? {
                generationId: storedTemplateSnapshot.generationId ?? null,
                templateId: storedTemplateSnapshot.resolvedTemplate?.id ?? null,
                templateVersion:
                  storedTemplateSnapshot.resolvedTemplate?.version ?? null,
                meetingContextId:
                  storedTemplateSnapshot.meetingContextId ?? null,
                meetingContextSha256:
                  storedTemplateSnapshot.meetingContextSha256 ?? null,
                summaryContextSha256:
                  storedTemplateSnapshot.summaryContextSha256 ?? null,
              }
            : null,
          storedFactValidation,
          storedFactValidationSha256: storedFact.sha256,
          storedFactValidationStatus: storedFact.status,
          storedFactWarningCodes: storedFact.warningCodes,
          storedFactContext: {
            meetingContextId: storedFact.meetingContextId,
            meetingContextSha256: storedFact.meetingContextSha256,
            summaryContextSha256: storedFact.summaryContextSha256,
          },
        };
      }),
    );
  const sanitizeSummary = (data) => {
    const summary = JSON.parse(JSON.stringify(data));
    delete summary.factValidation;
    delete summary.summary_json;
    delete summary.restoredRevisionId;
    return summary;
  };
  const recordingIdle = (recording) =>
    recording?.is_recording === false && recording?.is_active === false;
  const hasActiveGeneration = (history) =>
    history.some((item) =>
      ["pending", "processing"].includes(String(item?.status ?? "").toLowerCase()),
    );
  const read = async (command, payload = {}) => {
    if (!allowedReadOnlyCommands.has(command)) {
      throw new Error(`Read-only reopen runner blocked command: ${command}`);
    }
    commandTrace.push({ command, at: new Date().toISOString() });
    return invoke(command, payload);
  };
  const readState = async () => {
    const [meeting, summary, history, manualRevisions, recording, retranscription] =
      await Promise.all([
        read("api_get_meeting", { meetingId: config.meetingId }),
        read("api_get_summary", { meetingId: config.meetingId }),
        read("api_list_summary_generation_history", { meetingId: config.meetingId }),
        read("api_list_manual_summary_revisions", { meetingId: config.meetingId }),
        read("get_recording_state"),
        read("is_retranscription_in_progress_command"),
      ]);
    return { meeting, summary, history, manualRevisions, recording, retranscription };
  };
  const snapshotState = async (state) => {
    const markdown = state.summary?.data?.markdown ?? "";
    const factValidation = state.summary?.data?.factValidation ?? null;
    const templateSnapshot = state.summary?.data?.template_snapshot ?? null;
    const transcripts = transcriptProjection(state.meeting);
    const manualRevisions = await manualRevisionProjection(state.manualRevisions);
    const fact = await factValidationProjection(factValidation);
    const reconstructedPayload = {
      meetingId: config.meetingId,
      summary: sanitizeSummary(state.summary?.data ?? {}),
    };
    return {
      capturedAt: new Date().toISOString(),
      meeting: {
        id: state.meeting?.id ?? null,
        title: state.meeting?.title ?? null,
        createdAt: state.meeting?.created_at ?? null,
        updatedAt: state.meeting?.updated_at ?? null,
      },
      summary: {
        status: state.summary?.status ?? null,
        markdownLength: markdown.length,
        markdownSha256: await sha256Text(markdown),
        markdownProbe: pickMarkdownProbe(markdown),
        reconstructedSavePayloadSha256: await sha256Text(
          stableStringify(reconstructedPayload),
        ),
        summaryJsonPresent: Object.hasOwn(state.summary?.data ?? {}, "summary_json"),
        restoredRevisionId: state.summary?.data?.restoredRevisionId ?? null,
        dataKeys: Object.keys(state.summary?.data ?? {}).sort(),
        factValidation,
        factValidationSha256: fact.sha256,
        factValidationStatus: fact.status,
        factWarningCodes: fact.warningCodes,
        factContext: {
          meetingContextId: fact.meetingContextId,
          meetingContextSha256: fact.meetingContextSha256,
          summaryContextSha256: fact.summaryContextSha256,
        },
        templateSnapshotSha256: await sha256Text(stableStringify(templateSnapshot)),
        templateSnapshot: templateSnapshot
          ? {
              generationId: templateSnapshot.generationId ?? null,
              templateId: templateSnapshot.resolvedTemplate?.id ?? null,
              templateVersion: templateSnapshot.resolvedTemplate?.version ?? null,
              meetingContextId: templateSnapshot.meetingContextId ?? null,
              meetingContextSha256: templateSnapshot.meetingContextSha256 ?? null,
              summaryContextSha256: templateSnapshot.summaryContextSha256 ?? null,
            }
          : null,
      },
      transcripts: {
        count: transcripts.length,
        ids: transcripts.map((entry) => entry.id),
        concatenatedTextSha256: await sha256Text(
          transcripts.map((entry) => entry.text).join(""),
        ),
        projectionSha256: await sha256Text(stableStringify(transcripts)),
      },
      generationHistory: {
        count: state.history.length,
        sha256: await sha256Text(stableStringify(state.history)),
        entries: state.history,
      },
      manualRevisions: {
        projectionSchemaVersion: manualRevisionProjectionSchemaVersion,
        count: manualRevisions.length,
        sha256: await sha256Text(stableStringify(manualRevisions)),
        entries: manualRevisions,
      },
      recording: state.recording,
      recordingSha256: await sha256Text(stableStringify(state.recording)),
      retranscription: state.retranscription,
    };
  };

  let first = null;
  let actual = null;
  let stabilityGate = null;
  let editorEvidence = {
    candidateSelectors: summaryEditorCandidateSelectors,
    probe: {
      raw: String(config.expected.summary.markdownProbe ?? ""),
      normalized: normalizeVisibleText(config.expected.summary.markdownProbe),
      canonical: canonicalizeProbeText(config.expected.summary.markdownProbe),
    },
    initial: null,
    actual: null,
  };
  try {
    const initialEditorState = await waitForSummaryEditor(
      config.expected.summary.markdownProbe,
    );
    editorEvidence.initial = {
      containersFound: initialEditorState.containersFound,
      editorsFound: initialEditorState.editorsFound,
      probeMatches: initialEditorState.probeMatches,
      rawProbe: initialEditorState.rawProbe,
      canonicalProbe: initialEditorState.canonicalProbe,
      rawEditorTextTail: initialEditorState.selected.rawText.slice(-5000),
      canonicalEditorTextTail:
        initialEditorState.selected.canonicalText.slice(-5000),
    };

    // Only after the UI probe has settled do we read native state. Two
    // consecutive snapshots protect against history backfill or late page data.
    const firstRaw = await readState();
    first = await snapshotState(firstRaw);
    await new Promise((resolve) => setTimeout(resolve, 250));
    const secondRaw = await readState();
    actual = await snapshotState(secondRaw);
    stabilityGate = {
      meetingStable: stableStringify(actual.meeting) === stableStringify(first.meeting),
      markdownStable:
        actual.summary.markdownSha256 === first.summary.markdownSha256 &&
        actual.summary.markdownLength === first.summary.markdownLength,
      transcriptStable:
        actual.transcripts.projectionSha256 === first.transcripts.projectionSha256 &&
        actual.transcripts.concatenatedTextSha256 ===
          first.transcripts.concatenatedTextSha256,
      generationHistoryStable:
        actual.generationHistory.sha256 === first.generationHistory.sha256 &&
        actual.generationHistory.count === first.generationHistory.count,
      manualRevisionHistoryStable:
        actual.manualRevisions.projectionSchemaVersion ===
          first.manualRevisions.projectionSchemaVersion &&
        actual.manualRevisions.sha256 === first.manualRevisions.sha256,
      templateSnapshotStable:
        actual.summary.templateSnapshotSha256 ===
        first.summary.templateSnapshotSha256,
      factValidationStable:
        actual.summary.factValidationSha256 ===
        first.summary.factValidationSha256,
      contextStable:
        stableStringify(actual.summary.factContext) ===
          stableStringify(first.summary.factContext) &&
        stableStringify(actual.summary.templateSnapshot) ===
          stableStringify(first.summary.templateSnapshot),
      recordingStable:
        actual.recordingSha256 === first.recordingSha256,
      retranscriptionStable:
        actual.retranscription === first.retranscription,
    };
    if (!Object.values(stabilityGate).every(Boolean)) {
      throw new Error(
        `Two-snapshot read-only stability gate failed: ${JSON.stringify(stabilityGate)}`,
      );
    }

    const actualEditorState = findSummaryEditor(actual.summary.markdownProbe);
    if (!actualEditorState.selected) {
      throw new Error("Actual Markdown probe is not uniquely visible in the summary editor");
    }
    editorEvidence.actual = {
      containersFound: actualEditorState.containersFound,
      editorsFound: actualEditorState.editorsFound,
      probeMatches: actualEditorState.probeMatches,
      rawProbe: actualEditorState.rawProbe,
      canonicalProbe: actualEditorState.canonicalProbe,
      rawEditorTextTail: actualEditorState.selected.rawText.slice(-5000),
      canonicalEditorTextTail:
        actualEditorState.selected.canonicalText.slice(-5000),
    };
    const summaryArea =
      actualEditorState.selected.container.closest(".p-6.w-full") ??
      actualEditorState.selected.container.parentElement;
    if (!summaryArea || !summaryArea.contains(actualEditorState.selected.container)) {
      throw new Error("Summary area containing the BlockNote editor was not found");
    }
    const visibleFactBanners = [...summaryArea.querySelectorAll('[role="alert"]')]
      .filter(isVisible)
      .map((element) => normalizeVisibleText(element.innerText));
    const factBannerVisible = visibleFactBanners.length > 0;
    const expectedNeedsReview =
      config.expected.summary.factValidation?.status === "needs_review";
    const currentRevision = actual.manualRevisions.entries.find(
      (entry) => entry.revisionId === config.addedRevisionId,
    );
    const sourceGenerationId = config.expected.summary.templateSnapshot?.generationId;
    const sourceHistory = actual.generationHistory.entries.find(
      (entry) => entry?.generationId === sourceGenerationId,
    );
    const checks = {
      exactProductionMeetingUrl:
        location.href === config.destination &&
        location.origin === "http://tauri.localhost" &&
        location.pathname === "/meeting-details" &&
        new URL(location.href).searchParams.get("id") === config.meetingId &&
        new URL(location.href).searchParams.has("source") === false,
      routeTransitionVerified:
        config.mode !== "route" ||
        (config.routeTransitionEvidence?.routeLeftMeeting === true &&
          config.routeTransitionEvidence?.intermediateHref ===
            "http://tauri.localhost/" &&
          ["interactive", "complete"].includes(
            config.routeTransitionEvidence?.intermediateReadyState,
          ) &&
          config.routeTransitionEvidence?.intermediateTextLength >= 1 &&
          config.routeTransitionEvidence?.targetHref === config.destination &&
          ["interactive", "complete"].includes(
            config.routeTransitionEvidence?.targetReadyState,
          ) &&
          config.routeTransitionEvidence?.targetTextLength > 100),
      summaryEditorContainerFound:
        actualEditorState.selected !== null && actualEditorState.probeMatches === 1,
      summaryCompleted:
        actual.summary.status === config.expected.summary.status &&
        actual.summary.status === "completed",
      markdownPersistedByteForByte:
        actual.summary.markdownSha256 === config.expected.summary.markdownSha256 &&
        actual.summary.markdownLength === config.expected.summary.markdownLength,
      actualMarkdownProbeComputed:
        typeof actual.summary.markdownProbe === "string" &&
        actual.summary.markdownProbe.length > 0,
      markdownProbeMatchesBaseline:
        actual.summary.markdownProbe === config.expected.summary.markdownProbe,
      markdownVisibleOnlyInSummaryEditor:
        probeMatchesEditorText(
          actualEditorState.selected.rawText,
          actual.summary.markdownProbe,
        ),
      savePayloadHashReconstructed:
        actual.summary.reconstructedSavePayloadSha256 ===
        config.expectedSavePayloadSha256,
      canonicalSummaryJsonAbsent: actual.summary.summaryJsonPresent === false,
      restoredRevisionIdAbsent: actual.summary.restoredRevisionId === null,
      transcriptsUnchanged:
        actual.transcripts.count === config.expected.transcripts.count &&
        actual.transcripts.projectionSha256 ===
          config.expected.transcripts.projectionSha256 &&
        actual.transcripts.concatenatedTextSha256 ===
          config.expected.transcripts.concatenatedTextSha256 &&
        stableStringify(actual.transcripts.ids) ===
          stableStringify(config.expected.transcripts.ids),
      generationHistoryUnchanged:
        actual.generationHistory.count ===
          config.expected.generationHistory.count &&
        actual.generationHistory.sha256 ===
          config.expected.generationHistory.sha256,
      manualRevisionHistoryUnchanged:
        actual.manualRevisions.projectionSchemaVersion ===
          config.expected.manualRevisions.projectionSchemaVersion &&
        actual.manualRevisions.count === config.expected.manualRevisions.count &&
        actual.manualRevisions.sha256 === config.expected.manualRevisions.sha256,
      baselineAddedRevisionFound: Boolean(currentRevision),
      baselineAddedRevisionStillCurrent: currentRevision?.isCurrent === true,
      currentRevisionMarkdownMatchesCurrent:
        currentRevision?.markdownSha256 === actual.summary.markdownSha256,
      currentRevisionStoredFactMatchesBaselineAndDynamic:
        currentRevision?.storedFactValidationSha256 ===
          config.expectedRevision.storedFactValidationSha256 &&
        currentRevision?.storedFactValidationSha256 ===
          config.expectedNativeFactValidationSha256 &&
        currentRevision?.storedFactValidationSha256 ===
          actual.summary.factValidationSha256,
      currentRevisionSourceGenerationPreserved:
        currentRevision?.sourceGenerationId === sourceGenerationId,
      templateSnapshotPreserved:
        actual.summary.templateSnapshotSha256 ===
          config.expected.summary.templateSnapshotSha256 &&
        stableStringify(actual.summary.templateSnapshot) ===
          stableStringify(config.expected.summary.templateSnapshot),
      sourceGenerationStillPresentAndCurrent:
        Boolean(sourceHistory) && sourceHistory?.isCurrentSummary === true,
      factValidationPreserved:
        factValidationConsistent(actual.summary.factValidation) &&
        actual.summary.factValidationSha256 ===
          config.expected.summary.factValidationSha256,
      factContextPreserved:
        stableStringify(actual.summary.factContext) ===
          stableStringify(config.expected.summary.factContext),
      dynamicValidationAliasesNotClaimed:
        actual.summary.factValidation?.aliasesNormalized === false,
      factBannerExactlyMatchesStatus:
        factBannerVisible === expectedNeedsReview &&
        (expectedNeedsReview ? visibleFactBanners.length === 1 : true),
      meetingUpdatedAtPreserved:
        actual.meeting.updatedAt === config.expected.meeting.updatedAt,
      noActiveGeneration: !hasActiveGeneration(secondRaw.history),
      recordingIdle: recordingIdle(secondRaw.recording),
      retranscriptionIdle: secondRaw.retranscription === false,
      readOnlySnapshotsStable: Object.values(stabilityGate).every(Boolean),
    };
    const overallPass = Object.values(checks).every(Boolean);
    return {
      schemaVersion: 2,
      script: "cdp-uat06-revalidation-reopen",
      capturedAt: new Date().toISOString(),
      meetingId: config.meetingId,
      mode: config.mode,
      href: location.href,
      commitState: "not_attempted",
      baseline: {
        path: config.baselinePath,
        byteLength: config.baselineByteLength,
        fileSha256: config.baselineFileSha256,
        capturedAt: config.baselineCapturedAt,
        addedRevisionId: config.addedRevisionId,
        expectedSavePayloadSha256: config.expectedSavePayloadSha256,
        expectedNativeFactValidationSha256:
          config.expectedNativeFactValidationSha256,
      },
      routeTransitionEvidence: config.routeTransitionEvidence,
      first,
      actual,
      stabilityGate,
      editorEvidence,
      factUi: {
        expectedNeedsReview,
        factBannerVisible,
        visibleFactBanners,
      },
      commandTrace,
      verdict: { ...checks, overallPass },
    };
  } catch (error) {
    return {
      schemaVersion: 2,
      script: "cdp-uat06-revalidation-reopen",
      capturedAt: new Date().toISOString(),
      meetingId: config.meetingId,
      mode: config.mode,
      href: location.href,
      commitState: "not_attempted",
      baseline: {
        path: config.baselinePath,
        byteLength: config.baselineByteLength,
        fileSha256: config.baselineFileSha256,
        addedRevisionId: config.addedRevisionId,
        expectedSavePayloadSha256: config.expectedSavePayloadSha256,
        expectedNativeFactValidationSha256:
          config.expectedNativeFactValidationSha256,
      },
      routeTransitionEvidence: config.routeTransitionEvidence,
      error: {
        name: error instanceof Error ? error.name : "Error",
        message: error instanceof Error ? error.message : String(error),
      },
      first,
      actual,
      stabilityGate,
      editorEvidence,
      commandTrace,
      verdict: { overallPass: false },
    };
  }
}

const baselineIdentity = {
  path: path.resolve(baselinePath),
  byteLength: baselineBytes.length,
  fileSha256: baselineFileSha256,
  script: baseline.script,
  schemaVersion: baseline.schemaVersion,
  meetingId: baseline.meetingId,
  capturedAt: baseline.capturedAt ?? null,
  saveCommitState: baseline.commitState ?? null,
  saveOverallPass: baseline.verdict?.overallPass === true,
  frozenBaselineSha256: baseline.frozenBaseline?.sha256 ?? null,
  addedRevisionId,
  expectedSavePayloadSha256,
  expectedNativeFactValidationSha256,
  manualRevisionProjectionSchemaVersion:
    expected.manualRevisions.projectionSchemaVersion,
};
let routeTransitionEvidence = {
  routeLeftMeeting: mode !== "route" ? null : false,
  intermediateHref: null,
  intermediateReadyState: null,
  intermediateTextLength: null,
  intermediateWaitedMs: null,
  targetHref: null,
  targetReadyState: null,
  targetTextLength: null,
  targetWaitedMs: null,
  lastWait: null,
};
let socket = null;
let call = null;
let result = null;
let outerPhase = "cdp_target_discovery";
let lastPageWaitEvidence = null;

async function waitForPage(expectedUrl, minTextLength, timeoutMs = 30000) {
  if (typeof call !== "function") {
    throw new Error("CDP call transport is unavailable");
  }
  if (!Number.isSafeInteger(minTextLength) || minTextLength < 0) {
    throw new Error("waitForPage minTextLength must be a non-negative integer");
  }
  const started = Date.now();
  let state = null;
  while (Date.now() - started < timeoutMs) {
    const evaluated = await call("Runtime.evaluate", {
      expression:
        "({ href: location.href, origin: location.origin, readyState: document.readyState, text: document.body?.innerText ?? '' })",
      returnByValue: true,
    });
    if (evaluated.exceptionDetails) {
      throw new Error(
        evaluated.exceptionDetails.exception?.description ??
          evaluated.exceptionDetails.text,
      );
    }
    state = evaluated.result.value;
    const textLength = String(state?.text ?? "").trim().length;
    const ready = ["interactive", "complete"].includes(state?.readyState);
    lastPageWaitEvidence = {
      expectedUrl,
      minTextLength,
      href: state?.href ?? null,
      origin: state?.origin ?? null,
      readyState: state?.readyState ?? null,
      textLength,
      waitedMs: Date.now() - started,
    };
    if (
      state.origin === "http://tauri.localhost" &&
      state.href === expectedUrl &&
      ready &&
      textLength >= minTextLength
    ) {
      return { ...lastPageWaitEvidence };
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(
    `Timed out waiting for exact production page: ${expectedUrl}; minTextLength=${minTextLength}; last=${state?.href}; readyState=${state?.readyState}; textLength=${lastPageWaitEvidence?.textLength}`,
  );
}

try {
  const cdpPort = process.env.CDP_PORT ?? "9233";
  outerPhase = "cdp_target_discovery";
  const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`, {
    signal: AbortSignal.timeout(10000),
  }).then((response) => response.json());
  const page = targets.find(
    (target) =>
      target.type === "page" &&
      target.url.startsWith("http://tauri.localhost"),
  );
  if (!page) {
    throw new Error("Meetily production WebView2 debug target was not found");
  }

  outerPhase = "websocket_connect";
  socket = new WebSocket(page.webSocketDebuggerUrl);
  let nextId = 1;
  const pending = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    if (!message.id || !pending.has(message.id)) return;
    const { resolve, reject, timeout } = pending.get(message.id);
    pending.delete(message.id);
    clearTimeout(timeout);
    if (message.error) reject(new Error(JSON.stringify(message.error)));
    else resolve(message.result);
  });
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(
      () => reject(new Error("Timed out connecting to the WebView2 CDP socket")),
      10000,
    );
    const finish = (callback) => (event) => {
      clearTimeout(timeout);
      callback(event);
    };
    socket.addEventListener("open", finish(resolve), { once: true });
    socket.addEventListener("error", finish(reject), { once: true });
  });
  const rejectPending = (reason) => {
    for (const { reject, timeout } of pending.values()) {
      clearTimeout(timeout);
      reject(reason);
    }
    pending.clear();
  };
  socket.addEventListener("close", () =>
    rejectPending(new Error("WebView2 CDP socket closed")),
  );
  socket.addEventListener("error", () =>
    rejectPending(new Error("WebView2 CDP socket failed")),
  );
  call = (method, params = {}, timeoutMs = 15000) => {
    const id = nextId++;
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`Timed out waiting for CDP method ${method}`));
      }, timeoutMs);
      pending.set(id, { resolve, reject, timeout });
      try {
        socket.send(JSON.stringify({ id, method, params }));
      } catch (error) {
        clearTimeout(timeout);
        pending.delete(id);
        reject(error);
      }
    });
  };

  outerPhase = "cdp_enable";
  await call("Runtime.enable");
  await call("Page.enable");
  await call("Emulation.setDeviceMetricsOverride", {
    width: 1600,
    height: 1000,
    deviceScaleFactor: 1,
    mobile: false,
  });
  if (mode === "route") {
    outerPhase = "navigate_root";
    await call("Page.navigate", { url: "http://tauri.localhost/" });
    outerPhase = "wait_root";
    // The real root page is intentionally sparse. One visible character plus
    // exact production URL/origin and a ready document is enough to prove the
    // route left the meeting page.
    const intermediate = await waitForPage("http://tauri.localhost/", 1);
    routeTransitionEvidence = {
      ...routeTransitionEvidence,
      routeLeftMeeting: intermediate.href === "http://tauri.localhost/",
      intermediateHref: intermediate.href,
      intermediateReadyState: intermediate.readyState,
      intermediateTextLength: intermediate.textLength,
      intermediateWaitedMs: intermediate.waitedMs,
      lastWait: intermediate,
    };
  }
  outerPhase = "navigate_target";
  await call("Page.navigate", { url: destination });
  outerPhase = "wait_target";
  // Preserve the original target-page requirement of more than 100 visible
  // characters while allowing a different threshold for the sparse root page.
  const targetPage = await waitForPage(destination, 101);
  routeTransitionEvidence = {
    ...routeTransitionEvidence,
    targetHref: targetPage.href,
    targetReadyState: targetPage.readyState,
    targetTextLength: targetPage.textLength,
    targetWaitedMs: targetPage.waitedMs,
    lastWait: targetPage,
  };

  const config = {
    meetingId,
    mode,
    destination,
    expected,
    expectedRevision,
    addedRevisionId,
    expectedSavePayloadSha256,
    expectedNativeFactValidationSha256,
    baselinePath: path.resolve(baselinePath),
    baselineByteLength: baselineBytes.length,
    baselineFileSha256,
    baselineCapturedAt: baseline.capturedAt ?? null,
    routeTransitionEvidence,
    allowedReadOnlyCommands,
  };
  const expression = `(${runReopenAudit.toString()})(${JSON.stringify(config)})`;

  outerPhase = "run_reopen_audit";
  const evaluated = await call("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  }, 60000);
  if (evaluated.exceptionDetails) {
    throw new Error(
      evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text,
    );
  }
  result = evaluated.result.value;
  if (
    !result ||
    result.script !== "cdp-uat06-revalidation-reopen" ||
    result.schemaVersion !== 2 ||
    !result.verdict
  ) {
    throw new Error("runReopenAudit returned malformed or missing evidence");
  }
} catch (error) {
  routeTransitionEvidence = {
    ...routeTransitionEvidence,
    lastWait: lastPageWaitEvidence ?? routeTransitionEvidence.lastWait,
  };
  result = {
    schemaVersion: 2,
    script: "cdp-uat06-revalidation-reopen",
    capturedAt: new Date().toISOString(),
    meetingId,
    mode,
    commitState: "not_attempted",
    failureStage: outerPhase,
    baseline: baselineIdentity,
    routeTransitionEvidence,
    error: {
      name: error instanceof Error ? error.name : "Error",
      message: error instanceof Error ? error.message : String(error),
    },
    commandTrace: null,
    verdict: { overallPass: false },
  };
}

if (mode === "current") {
  const processPathTransport = "base64_utf8";
  try {
    const processExists = (pid) => {
    try {
      process.kill(pid, 0);
      return true;
    } catch (error) {
      if (error?.code === "ESRCH") return false;
      if (error?.code === "EPERM") return true;
      throw error;
    }
  };
  const decodeProcessPathBase64Utf8 = (
    encodedValue,
    pathExists = fs.existsSync,
  ) => {
    const encoded = String(encodedValue ?? "").trim();
    // An absent old PID intentionally produces empty stdout and maps to null.
    if (!encoded) return null;
    const strictBase64 =
      /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;
    if (!strictBase64.test(encoded)) {
      throw new Error("PowerShell process path transport returned invalid Base64");
    }
    const bytes = Buffer.from(encoded, "base64");
    if (bytes.toString("base64") !== encoded) {
      throw new Error("PowerShell process path Base64 failed strict round-trip validation");
    }
    const decoded = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    if (!decoded || decoded.includes("\0") || !path.isAbsolute(decoded)) {
      throw new Error("Decoded process path is not a valid absolute path");
    }
    if (!pathExists(decoded)) {
      throw new Error(`Decoded process path does not exist: ${decoded}`);
    }
    return decoded;
  };
  const processPath = (pid) => {
    const script = [
      `$targetProcess = Get-Process -Id ${pid} -ErrorAction SilentlyContinue`,
      "if ($null -eq $targetProcess) { exit 0 }",
      "$targetPath = $targetProcess.Path",
      "if ([string]::IsNullOrWhiteSpace($targetPath)) { exit 0 }",
      "$utf8Bytes = [Text.Encoding]::UTF8.GetBytes($targetPath)",
      "[Console]::Out.Write([Convert]::ToBase64String($utf8Bytes))",
    ].join("; ");
    const encodedPath = execFileSync(
      "powershell.exe",
      ["-NoProfile", "-NonInteractive", "-Command", script],
      {
        encoding: "ascii",
        windowsHide: true,
        stdio: ["ignore", "pipe", "ignore"],
      },
    );
    return decodeProcessPathBase64Utf8(encodedPath);
  };
  const canonicalPath = (value) => {
    if (!value) return null;
    try {
      return fs.realpathSync.native(value).toLowerCase();
    } catch {
      return path.resolve(value).toLowerCase();
    }
  };
  const sha256File = (value) =>
    crypto.createHash("sha256").update(fs.readFileSync(value)).digest("hex").toUpperCase();
  const oldProcessPath = processPath(pidBefore);
  const newProcessPath = processPath(pidAfter);
  const configuredExecutableSha256 = sha256File(executablePath);
  const runningExecutableSha256 = newProcessPath ? sha256File(newProcessPath) : null;
  result.restartMetadata = {
    pidBefore,
    pidAfter,
    pidChanged: pidBefore !== pidAfter,
    oldProcessGone: processExists(pidBefore) === false,
    newProcessAlive: processExists(pidAfter) === true,
    oldProcessPath,
    newProcessPath,
    configuredExecutablePath: path.resolve(executablePath),
    processPathMatchesConfigured:
      canonicalPath(newProcessPath) === canonicalPath(executablePath),
    configuredExecutableSha256,
    runningExecutableSha256,
    expectedReleaseSha256,
    processPathTransport,
    configuredHashMatchesExpected:
      configuredExecutableSha256 === expectedReleaseSha256,
    runningHashMatchesExpected: runningExecutableSha256 === expectedReleaseSha256,
  };
  const coreAuditPassed = result?.verdict?.overallPass === true;
  const restartChecks = {
    restartPidChanged: result.restartMetadata.pidChanged,
    oldProcessExited: result.restartMetadata.oldProcessGone,
    newProcessIsAlive: result.restartMetadata.newProcessAlive,
    runningProcessUsesExpectedExecutable:
      result.restartMetadata.processPathMatchesConfigured,
    configuredExecutableHashMatches:
      result.restartMetadata.configuredHashMatchesExpected,
    runningExecutableHashMatches: result.restartMetadata.runningHashMatchesExpected,
  };
  Object.assign(result.verdict, { coreAuditPassed, ...restartChecks });
  result.verdict.overallPass =
    coreAuditPassed && Object.values(restartChecks).every((value) => value === true);
  } catch (error) {
    result.restartMetadata = {
      pidBefore,
      pidAfter,
      configuredExecutablePath: path.resolve(executablePath),
      expectedReleaseSha256,
      processPathTransport,
      error: error instanceof Error ? error.message : String(error),
    };
    const coreAuditPassed = result?.verdict?.overallPass === true;
    Object.assign(result.verdict, {
      coreAuditPassed,
      restartPidChanged: false,
      oldProcessExited: false,
      newProcessIsAlive: false,
      runningProcessUsesExpectedExecutable: false,
      configuredExecutableHashMatches: false,
      runningExecutableHashMatches: false,
      overallPass: false,
    });
  }
}

if (screenshotPath) {
  if (typeof call !== "function" || socket?.readyState !== 1) {
    result.screenshot = {
      path: path.resolve(screenshotPath),
      captured: false,
      error: "CDP screenshot transport is unavailable",
    };
  } else {
    try {
      const screenshot = await call("Page.captureScreenshot", { format: "png" });
      fs.mkdirSync(path.dirname(path.resolve(screenshotPath)), { recursive: true });
      fs.writeFileSync(screenshotPath, Buffer.from(screenshot.data, "base64"), {
        flag: "wx",
      });
      result.screenshot = { path: path.resolve(screenshotPath), captured: true };
    } catch (error) {
      result.screenshot = {
        path: path.resolve(screenshotPath),
        captured: false,
        error: error instanceof Error ? error.message : String(error),
      };
    }
  }
}
if (socket) {
  try {
    socket.close();
  } catch {
    // Evidence writing must not be suppressed by a failed best-effort close.
  }
}

const output = `${JSON.stringify(result, null, 2)}\n`;
fs.mkdirSync(path.dirname(path.resolve(outputPath)), { recursive: true });
fs.writeFileSync(outputPath, output, { encoding: "utf8", flag: "wx" });
console.log(output.trimEnd());
if (result?.verdict?.overallPass !== true) {
  throw new Error(`UAT-06 revalidation reopen failed; evidence written to ${outputPath}`);
}
