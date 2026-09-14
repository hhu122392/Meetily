import fs from "node:fs";
import path from "node:path";

const meetingId = process.argv[2];
const expectedMarkdownSha256 = (process.argv[3] ?? "").toLowerCase();
const expectedTranscriptSha256 = (process.argv[4] ?? "").toLowerCase();
const outputPath = process.argv[5];
const screenshotPath = process.argv[6];
if (!meetingId || !expectedMarkdownSha256 || !expectedTranscriptSha256 || !outputPath) {
  throw new Error(
    "Usage: node cdp-uat06-revalidation-save.mjs <meeting-id> <expected-markdown-sha256> <expected-transcript-sha256> <output.json> [screenshot.png]",
  );
}

const sha256Pattern = /^[a-f0-9]{64}$/;
const identifierPattern = /^[A-Za-z0-9][A-Za-z0-9._:-]{2,255}$/;
if (!identifierPattern.test(meetingId)) {
  throw new Error("Safety gate closed: meeting id has an invalid format");
}
if (!sha256Pattern.test(expectedMarkdownSha256)) {
  throw new Error("Safety gate closed: expected Markdown SHA-256 is invalid");
}
if (!sha256Pattern.test(expectedTranscriptSha256)) {
  throw new Error("Safety gate closed: expected transcript SHA-256 is invalid");
}

const confirmedMeetingId = process.env.UAT_IDENTICAL_SAVE_CONFIRM ?? "";
const expectedSourceGenerationId = process.env.UAT_EXPECTED_SOURCE_GENERATION_ID ?? "";
const expectedRestoredRevisionId = process.env.UAT_EXPECTED_RESTORED_REVISION_ID ?? "";
const expectedHistoryCountRaw = process.env.UAT_EXPECTED_HISTORY_COUNT ?? "";
const expectedRevisionCountRaw = process.env.UAT_EXPECTED_MANUAL_REVISION_COUNT ?? "";
if (confirmedMeetingId !== meetingId) {
  throw new Error(
    "Safety gate closed: UAT_IDENTICAL_SAVE_CONFIRM must equal the exact meeting id",
  );
}
if (!identifierPattern.test(expectedSourceGenerationId)) {
  throw new Error(
    "Safety gate closed: UAT_EXPECTED_SOURCE_GENERATION_ID has an invalid format",
  );
}
if (expectedRestoredRevisionId && !identifierPattern.test(expectedRestoredRevisionId)) {
  throw new Error(
    "Safety gate closed: UAT_EXPECTED_RESTORED_REVISION_ID has an invalid format",
  );
}
const parseExpectedCount = (value, name, minimum) => {
  if (!/^\d+$/.test(value)) {
    throw new Error(`Safety gate closed: ${name} must be a non-negative integer`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < minimum) {
    throw new Error(`Safety gate closed: ${name} is outside the accepted range`);
  }
  return parsed;
};
const expectedHistoryCount = parseExpectedCount(
  expectedHistoryCountRaw,
  "UAT_EXPECTED_HISTORY_COUNT",
  1,
);
const expectedRevisionCount = parseExpectedCount(
  expectedRevisionCountRaw,
  "UAT_EXPECTED_MANUAL_REVISION_COUNT",
  0,
);
if (fs.existsSync(outputPath)) {
  throw new Error(`Refusing to overwrite existing evidence: ${outputPath}`);
}
if (screenshotPath && fs.existsSync(screenshotPath)) {
  throw new Error(`Refusing to overwrite existing screenshot: ${screenshotPath}`);
}

const allowedReadOnlyCommands = [
  "api_get_meeting",
  "api_get_summary",
  "api_list_summary_generation_history",
  "api_list_manual_summary_revisions",
  "get_recording_state",
  "is_retranscription_in_progress_command",
];
const oneTimeWriteCommand = "api_save_meeting_summary";
const knownForbiddenCommands = [
  "api_process_transcript",
  "api_cancel_summary",
  "api_restore_manual_summary_revision",
  "api_update_transcript_segment",
  "api_delete_transcript_segment",
  "api_delete_meeting",
  "delete_meeting",
  "start_recording",
  "start_recording_with_devices",
  "stop_recording",
  "pause_recording",
  "resume_recording",
  "start_retranscription",
  "start_retranscription_command",
  "cancel_retranscription",
  "cancel_retranscription_command",
  "api_create_template_v2",
  "api_update_template_v2",
  "api_delete_template_v2",
  "api_set_model_config",
  "api_save_transcript_config",
];

async function runSaveAudit(config) {
  const invokeBridge = window.__TAURI_INTERNALS__;
  if (typeof invokeBridge?.invoke !== "function") {
    return {
      schemaVersion: 2,
      script: "cdp-uat06-revalidation-save",
      meetingId: config.meetingId,
      capturedAt: new Date().toISOString(),
      commitState: "not_attempted",
      error: { message: "Tauri invoke bridge is unavailable" },
      commandTrace: { runnerCalls: [], bridgeCalls: [], blockedCalls: [] },
      verdict: { overallPass: false },
    };
  }

  const originalInvoke = invokeBridge.invoke;
  const allowedReadOnlyCommands = new Set(config.allowedReadOnlyCommands);
  const knownForbiddenCommands = new Set(config.knownForbiddenCommands);
  const bridgeCalls = [];
  const runnerCalls = [];
  const blockedCalls = [];
  let armedSavePayload = null;
  let bridgeSaveAttempts = 0;
  let runnerSaveAttempts = 0;
  let runnerSavePayloadSha256 = null;
  let bridgeSavePayloadSha256 = null;
  let savePayloadObjectIdentityVerified = false;
  let commitState = "not_attempted";
  let first = null;
  let confirmation = null;
  let provenanceTransition = null;
  let frozenBaselineSha256 = null;
  let candidatePayloadSha256 = null;
  let savePayloadSha256 = null;
  let preflight = null;
  let stabilityGate = null;

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
  const recordingIdle = (recording) =>
    recording?.is_recording === false && recording?.is_active === false;
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
  const manualImmutableProjection = (entries) =>
    entries.map(({ isCurrent: _ignored, ...entry }) => entry);
  const sanitizeSummary = (data) => {
    const summary = JSON.parse(JSON.stringify(data));
    delete summary.factValidation;
    delete summary.summary_json;
    delete summary.restoredRevisionId;
    return summary;
  };
  const restoredProvenanceObservation = (rawSummary, snapshot) => {
    const currentEntries = snapshot.manualRevisions.entries.filter(
      (entry) => entry.isCurrent === true,
    );
    const expectedEntry = config.expectedRestoredRevisionId
      ? snapshot.manualRevisions.entries.find(
          (entry) => entry.revisionId === config.expectedRestoredRevisionId,
        ) ?? null
      : null;
    const expectedRestoredRevisionMatchesCurrent = config.expectedRestoredRevisionId
      ? currentEntries.length === 1 &&
        currentEntries[0]?.revisionId === config.expectedRestoredRevisionId &&
        expectedEntry?.markdownSha256 === snapshot.summary.markdownSha256 &&
        expectedEntry?.storedTemplateSnapshotSha256 ===
          snapshot.summary.templateSnapshotSha256 &&
        expectedEntry?.sourceGenerationId === config.expectedSourceGenerationId &&
        expectedEntry?.storedTemplateSnapshot?.generationId ===
          config.expectedSourceGenerationId &&
        snapshot.summary.templateSnapshot?.generationId ===
          config.expectedSourceGenerationId
      : null;
    return {
      // api_get_summary deliberately strips the persisted SQL provenance
      // marker during revalidation. The revision list is authoritative here.
      getterMarkerExposed: Object.hasOwn(
        rawSummary?.data ?? {},
        "restoredRevisionId",
      ),
      getterMarkerValue: snapshot?.summary?.restoredRevisionId ?? null,
      currentRevisionIds: currentEntries.map((entry) => entry.revisionId),
      expectedRevision: expectedEntry
        ? {
            revisionId: expectedEntry.revisionId,
            isCurrent: expectedEntry.isCurrent,
            markdownSha256: expectedEntry.markdownSha256,
            sourceGenerationId: expectedEntry.sourceGenerationId,
            storedTemplateSnapshotSha256:
              expectedEntry.storedTemplateSnapshotSha256,
            storedTemplateSnapshot: expectedEntry.storedTemplateSnapshot,
          }
        : null,
      currentSummary: {
        markdownSha256: snapshot.summary.markdownSha256,
        templateSnapshotSha256: snapshot.summary.templateSnapshotSha256,
        sourceGenerationId:
          snapshot.summary.templateSnapshot?.generationId ?? null,
      },
      expectedRestoredRevisionMatchesCurrent,
    };
  };
  const sourceHistoryItem = (snapshot) =>
    snapshot.generationHistory.entries.find(
      (item) => item?.generationId === config.expectedSourceGenerationId,
    ) ?? null;
  const frozenMatches = (snapshot) => ({
    markdownSha256Matches: snapshot.summary.markdownSha256 === config.expectedMarkdownSha256,
    transcriptSha256Matches:
      snapshot.transcripts.concatenatedTextSha256 === config.expectedTranscriptSha256,
    sourceGenerationMatches:
      snapshot.summary.templateSnapshot?.generationId === config.expectedSourceGenerationId &&
      Boolean(sourceHistoryItem(snapshot)),
    sourceGenerationIsCurrent:
      sourceHistoryItem(snapshot)?.isCurrentSummary === true,
    historyCountMatches: snapshot.generationHistory.count === config.expectedHistoryCount,
    manualRevisionCountMatches:
      snapshot.manualRevisions.count === config.expectedRevisionCount,
  });
  const hasActiveGeneration = (history) =>
    history.some((item) =>
      ["pending", "processing"].includes(String(item?.status ?? "").toLowerCase()),
    );
  const summarySaveButtonState = () => {
    const buttons = [...document.querySelectorAll("button")];
    const copyButtons = buttons.filter((button) => {
      const label = [button.getAttribute("aria-label"), button.title, button.innerText]
        .filter(Boolean)
        .join(" ");
      return /复制摘要|Copy summary/i.test(label);
    });
    const updaterGroups = [
      ...new Set(
        copyButtons
          .map((button) => button.closest('[data-slot="button-group"]'))
          .filter(Boolean),
      ),
    ];
    const groupedCandidates = updaterGroups
      .map((group) => group.querySelector(":scope > button:first-of-type"))
      .filter(Boolean);
    const labeledCandidates = buttons.filter((button) => {
      const label = [button.getAttribute("aria-label"), button.title, button.innerText]
        .filter(Boolean)
        .join(" ");
      return /保存更改|Save changes/i.test(label);
    });
    const candidates = [...new Set([...groupedCandidates, ...labeledCandidates])];
    const described = candidates.map((button) => ({
      identifiedByUpdaterGroup: groupedCandidates.includes(button),
      ariaLabel: button.getAttribute("aria-label"),
      title: button.title || null,
      text: button.innerText.trim(),
      className: String(button.className ?? ""),
      disabled: button.disabled,
      hasSpinner: Boolean(button.querySelector(".animate-spin")),
      dirty: String(button.className ?? "").split(/\s+/).includes("bg-green-200"),
      saving: button.disabled || Boolean(button.querySelector(".animate-spin")),
    }));
    return {
      foundExactlyOne: candidates.length === 1,
      candidates: described,
      dirty: described.some((item) => item.dirty),
      saving: described.some((item) => item.saving),
    };
  };

  const guardedRead = async (command, payload = {}) => {
    const callRecord = {
      command,
      at: new Date().toISOString(),
      payloadKeys:
        payload && typeof payload === "object" ? Object.keys(payload).sort() : [],
      payloadSha256: await sha256Text(stableStringify(payload)),
    };
    if (!allowedReadOnlyCommands.has(command)) {
      blockedCalls.push({
        ...callRecord,
        scope: "runner",
        reason: knownForbiddenCommands.has(command)
          ? "known_forbidden_command"
          : "runner_command_not_on_explicit_allowlist",
      });
      throw new Error(`Runner attempted a non-read command: ${command}`);
    }
    runnerCalls.push({ ...callRecord, kind: "read" });
    bridgeCalls.push({
      ...callRecord,
      kind: "read",
      invokedVia: "originalInvoke.call",
    });
    return originalInvoke.call(invokeBridge, command, payload);
  };
  const guardedIdenticalSave = async (
    command,
    payload,
    expectedCompletePayloadSha256,
  ) => {
    runnerSaveAttempts += 1;
    runnerSavePayloadSha256 = await sha256Text(stableStringify(payload));
    const runnerCall = {
      command,
      at: new Date().toISOString(),
      kind: "one_time_write",
      attempt: runnerSaveAttempts,
      payloadKeys:
        payload && typeof payload === "object" ? Object.keys(payload).sort() : [],
      payloadSha256: runnerSavePayloadSha256,
      payloadObjectMatchesArmed: payload === armedSavePayload,
    };
    runnerCalls.push(runnerCall);
    const runnerGate = {
      commandMatches: command === config.oneTimeWriteCommand,
      oneRunnerAttempt: runnerSaveAttempts === 1,
      payloadObjectMatchesArmed: payload === armedSavePayload,
      completePayloadHashMatches:
        runnerSavePayloadSha256 === expectedCompletePayloadSha256,
    };
    if (!Object.values(runnerGate).every(Boolean)) {
      blockedCalls.push({
        ...runnerCall,
        scope: "runner",
        reason: "runner_one_time_save_gate_failed",
        gate: runnerGate,
      });
      throw new Error(
        `Runner one-time save gate failed: ${JSON.stringify(runnerGate)}`,
      );
    }

    // WebView2 exposes __TAURI_INTERNALS__.invoke as a non-writable,
    // non-configurable property. Do not replace it: independently re-check the
    // exact command, complete payload hash, object identity and bridge budget,
    // then make the runner's sole direct native write.
    const bridgePayload = payload;
    bridgeSavePayloadSha256 = await sha256Text(stableStringify(bridgePayload));
    const bridgeGate = {
      commandMatches: command === config.oneTimeWriteCommand,
      nextBridgeAttemptIsFirst: bridgeSaveAttempts + 1 === 1,
      payloadObjectMatchesRunner: bridgePayload === payload,
      payloadObjectMatchesArmed: bridgePayload === armedSavePayload,
      completePayloadHashMatchesRunner:
        bridgeSavePayloadSha256 === runnerSavePayloadSha256,
      completePayloadHashMatchesExpected:
        bridgeSavePayloadSha256 === expectedCompletePayloadSha256,
    };
    savePayloadObjectIdentityVerified =
      bridgeGate.payloadObjectMatchesRunner &&
      bridgeGate.payloadObjectMatchesArmed;
    const bridgeCall = {
      command,
      at: new Date().toISOString(),
      kind: "one_time_write",
      attempt: bridgeSaveAttempts + 1,
      payloadKeys:
        bridgePayload && typeof bridgePayload === "object"
          ? Object.keys(bridgePayload).sort()
          : [],
      payloadSha256: bridgeSavePayloadSha256,
      payloadObjectMatchesRunner: bridgeGate.payloadObjectMatchesRunner,
      payloadObjectMatchesArmed: bridgeGate.payloadObjectMatchesArmed,
      invokedVia: "originalInvoke.call",
    };
    if (!Object.values(bridgeGate).every(Boolean)) {
      blockedCalls.push({
        ...bridgeCall,
        scope: "runner_bridge_boundary",
        reason: "bridge_one_time_save_gate_failed",
        gate: bridgeGate,
      });
      throw new Error(
        `Bridge one-time save gate failed: ${JSON.stringify(bridgeGate)}`,
      );
    }
    bridgeSaveAttempts += 1;
    bridgeCalls.push(bridgeCall);
    commitState = "uncertain";
    return originalInvoke.call(invokeBridge, command, bridgePayload);
  };
  const readState = async () => {
    const [meeting, summary, history, manualRevisions, recording, retranscription] =
      await Promise.all([
        guardedRead("api_get_meeting", { meetingId: config.meetingId }),
        guardedRead("api_get_summary", { meetingId: config.meetingId }),
        guardedRead("api_list_summary_generation_history", {
          meetingId: config.meetingId,
        }),
        guardedRead("api_list_manual_summary_revisions", {
          meetingId: config.meetingId,
        }),
        guardedRead("get_recording_state"),
        guardedRead("is_retranscription_in_progress_command"),
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
        immutableSha256: await sha256Text(
          stableStringify(manualImmutableProjection(manualRevisions)),
        ),
        entries: manualRevisions,
      },
      recording: state.recording,
      recordingSha256: await sha256Text(stableStringify(state.recording)),
      retranscription: state.retranscription,
    };
  };

  try {
    frozenBaselineSha256 = await sha256Text(
      stableStringify({
        meetingId: config.meetingId,
        markdownSha256: config.expectedMarkdownSha256,
        transcriptSha256: config.expectedTranscriptSha256,
        sourceGenerationId: config.expectedSourceGenerationId,
        restoredRevisionId: config.expectedRestoredRevisionId || null,
        historyCount: config.expectedHistoryCount,
        manualRevisionCount: config.expectedRevisionCount,
      }),
    );

    const firstRaw = await readState();
    first = await snapshotState(firstRaw);
    provenanceTransition = {
      expectedBeforeRestoredRevisionId:
        config.expectedRestoredRevisionId || null,
      expectedTransition: config.expectedRestoredRevisionId
        ? "restored_revision_to_fresh_manual_save"
        : "canonical_summary_to_fresh_manual_save",
      getterMarkerExpectedExposed: false,
      authoritativeProvenanceSource: "api_list_manual_summary_revisions",
      before: restoredProvenanceObservation(firstRaw.summary, first),
      confirmation: null,
      payload: null,
      response: null,
      after: null,
      expectedTransitionSatisfied: null,
    };
    const firstSaveButton = summarySaveButtonState();
    const firstFrozen = frozenMatches(first);
    const firstFactValidation = firstRaw.summary?.data?.factValidation ?? null;
    preflight = {
      productionOrigin: location.origin === "http://tauri.localhost",
      exactMeetingRoute:
        location.pathname === "/meeting-details" &&
        new URL(location.href).searchParams.get("id") === config.meetingId,
      completedSummary: firstRaw.summary?.status === "completed",
      markdownAvailable:
        typeof firstRaw.summary?.data?.markdown === "string" &&
        firstRaw.summary.data.markdown.length > 0,
      canonicalDynamicGetHasNoSummaryJson: !Object.hasOwn(
        firstRaw.summary?.data ?? {},
        "summary_json",
      ),
      getterRestoredRevisionMarkerHidden:
        provenanceTransition.before.getterMarkerExposed === false &&
        provenanceTransition.before.getterMarkerValue === null,
      restoredRevisionProvenanceExpected: config.expectedRestoredRevisionId
        ? provenanceTransition.before.expectedRestoredRevisionMatchesCurrent === true
        : true,
      dynamicValidationConsistent: factValidationConsistent(firstFactValidation),
      dynamicValidationAliasesNotClaimed:
        firstFactValidation?.aliasesNormalized === false,
      currentContextHashesAvailable: [
        firstFactValidation?.meetingContextId,
        firstFactValidation?.meetingContextSha256,
        firstFactValidation?.summaryContextSha256,
      ].every((value) => typeof value === "string" && value.length > 0),
      templateSnapshotAvailable: Boolean(firstRaw.summary?.data?.template_snapshot),
      sourceGenerationFrozen: firstFrozen.sourceGenerationMatches,
      sourceGenerationCurrent: firstFrozen.sourceGenerationIsCurrent,
      markdownFrozen: firstFrozen.markdownSha256Matches,
      transcriptFrozen: firstFrozen.transcriptSha256Matches,
      historyCountFrozen: firstFrozen.historyCountMatches,
      manualRevisionCountFrozen: firstFrozen.manualRevisionCountMatches,
      manualRevisionListBelowApiLimit: firstRaw.manualRevisions.length < 200,
      noActiveGeneration: !hasActiveGeneration(firstRaw.history),
      recordingIdle: recordingIdle(firstRaw.recording),
      retranscriptionIdle: firstRaw.retranscription === false,
      summarySaveButtonFound: firstSaveButton.foundExactlyOne,
      summaryNotDirty: firstSaveButton.dirty === false,
      summaryNotSaving: firstSaveButton.saving === false,
      noBusyUi: document.querySelectorAll('[aria-busy="true"]').length === 0,
      noVisibleDialog: [...document.querySelectorAll('[role="dialog"]')].every(
        (element) => {
          const rect = element.getBoundingClientRect();
          return rect.width === 0 || rect.height === 0;
        },
      ),
      noRunnerCommandBlockedBeforeSave: blockedCalls.length === 0,
      noSaveAttemptBeforeArming: bridgeSaveAttempts === 0 && runnerSaveAttempts === 0,
    };
    if (!Object.values(preflight).every(Boolean)) {
      throw new Error(`Pre-save safety gate failed: ${JSON.stringify(preflight)}`);
    }

    const candidateSummary = sanitizeSummary(firstRaw.summary.data);
    const candidatePayload = { meetingId: config.meetingId, summary: candidateSummary };
    candidatePayloadSha256 = await sha256Text(stableStringify(candidatePayload));

    await new Promise((resolve) => setTimeout(resolve, 250));
    const confirmationRaw = await readState();
    confirmation = await snapshotState(confirmationRaw);
    provenanceTransition = {
      ...provenanceTransition,
      confirmation: restoredProvenanceObservation(
        confirmationRaw.summary,
        confirmation,
      ),
    };
    const confirmationFrozen = frozenMatches(confirmation);
    const confirmationSaveButton = summarySaveButtonState();
    const confirmationFactValidation =
      confirmationRaw.summary?.data?.factValidation ?? null;
    stabilityGate = {
      markdownStable:
        confirmation.summary.markdownSha256 === first.summary.markdownSha256 &&
        confirmation.summary.markdownLength === first.summary.markdownLength,
      transcriptStable:
        confirmation.transcripts.projectionSha256 === first.transcripts.projectionSha256 &&
        confirmation.transcripts.concatenatedTextSha256 ===
          first.transcripts.concatenatedTextSha256,
      generationHistoryStable:
        confirmation.generationHistory.sha256 === first.generationHistory.sha256,
      manualRevisionsStable:
        confirmation.manualRevisions.sha256 === first.manualRevisions.sha256,
      templateSnapshotStable:
        confirmation.summary.templateSnapshotSha256 ===
        first.summary.templateSnapshotSha256,
      factValidationStable:
        confirmation.summary.factValidationSha256 ===
        first.summary.factValidationSha256,
      contextStable:
        stableStringify(confirmation.summary.factContext) ===
          stableStringify(first.summary.factContext) &&
        stableStringify(confirmation.summary.templateSnapshot) ===
          stableStringify(first.summary.templateSnapshot),
      meetingUpdatedAtStable:
        confirmation.meeting.updatedAt === first.meeting.updatedAt,
      recordingStableAndIdle:
        confirmation.recordingSha256 === first.recordingSha256 &&
        recordingIdle(confirmationRaw.recording),
      retranscriptionStableAndIdle:
        confirmation.retranscription === first.retranscription &&
        confirmationRaw.retranscription === false,
      summaryStatusStable:
        confirmation.summary.status === first.summary.status &&
        confirmation.summary.status === "completed",
      getterRestoredRevisionMarkerStillHidden:
        provenanceTransition.confirmation.getterMarkerExposed === false &&
        provenanceTransition.confirmation.getterMarkerValue === null,
      restoredRevisionProvenanceStableAndExpected:
        config.expectedRestoredRevisionId
          ? provenanceTransition.before.expectedRestoredRevisionMatchesCurrent ===
              true &&
            provenanceTransition.confirmation
              .expectedRestoredRevisionMatchesCurrent === true &&
            stableStringify(
              provenanceTransition.confirmation.currentRevisionIds,
            ) ===
              stableStringify(provenanceTransition.before.currentRevisionIds)
          : true,
      dynamicValidationStillConsistent:
        factValidationConsistent(confirmationFactValidation),
      markdownStillFrozen: confirmationFrozen.markdownSha256Matches,
      transcriptStillFrozen: confirmationFrozen.transcriptSha256Matches,
      sourceGenerationStillFrozen:
        confirmationFrozen.sourceGenerationMatches &&
        confirmationFrozen.sourceGenerationIsCurrent,
      historyCountStillFrozen: confirmationFrozen.historyCountMatches,
      manualRevisionCountStillFrozen:
        confirmationFrozen.manualRevisionCountMatches,
      summarySaveButtonStillFound: confirmationSaveButton.foundExactlyOne,
      summaryStillNotDirty: confirmationSaveButton.dirty === false,
      summaryStillNotSaving: confirmationSaveButton.saving === false,
      noRunnerCommandBlockedBeforeCommit: blockedCalls.length === 0,
      noSaveAttemptBeforeCommit: bridgeSaveAttempts === 0 && runnerSaveAttempts === 0,
    };
    if (!Object.values(stabilityGate).every(Boolean)) {
      throw new Error(
        `Pre-commit stability gate failed: ${JSON.stringify(stabilityGate)}`,
      );
    }

    const summaryToSave = sanitizeSummary(confirmationRaw.summary.data);
    const savePayload = { meetingId: config.meetingId, summary: summaryToSave };
    savePayloadSha256 = await sha256Text(stableStringify(savePayload));
    provenanceTransition = {
      ...provenanceTransition,
      payload: {
        propertyPresent: Object.hasOwn(summaryToSave, "restoredRevisionId"),
        value: summaryToSave.restoredRevisionId ?? null,
      },
    };
    if (
      summaryToSave.markdown !== confirmationRaw.summary.data.markdown ||
      savePayloadSha256 !== candidatePayloadSha256
    ) {
      throw new Error("Final payload is not identical to the twice-confirmed summary");
    }

    // guardedIdenticalSave changes commitState to `uncertain` immediately before
    // its sole originalInvoke.call. A local gate failure remains not_attempted;
    // a lost native response is never retried.
    armedSavePayload = savePayload;
    let saveResponse;
    try {
      saveResponse = await guardedIdenticalSave(
        config.oneTimeWriteCommand,
        savePayload,
        savePayloadSha256,
      );
    } finally {
      armedSavePayload = null;
    }
    const afterRaw = await readState();
    const after = await snapshotState(afterRaw);
    const beforeRevisionIds = new Set(
      confirmation.manualRevisions.entries.map((entry) => entry.revisionId),
    );
    const addedRevisionIds = after.manualRevisions.entries
      .map((entry) => entry.revisionId)
      .filter((revisionId) => !beforeRevisionIds.has(revisionId));
    const addedRevisionId = addedRevisionIds.length === 1 ? addedRevisionIds[0] : null;
    const addedRevision = after.manualRevisions.entries.find(
      (entry) => entry.revisionId === addedRevisionId,
    );
    const oldAfterEntries = after.manualRevisions.entries.filter((entry) =>
      beforeRevisionIds.has(entry.revisionId),
    );
    const responseSummary = saveResponse?.summary ?? null;
    const responseFactValidation = saveResponse?.factValidation ?? null;
    const responseFactValidationSha256 = await sha256Text(
      stableStringify(responseFactValidation),
    );
    const responseSummaryFactValidationSha256 = await sha256Text(
      stableStringify(responseSummary?.factValidation ?? null),
    );
    const responseMarkdownSha256 = await sha256Text(responseSummary?.markdown ?? "");
    const afterFactValidation = afterRaw.summary?.data?.factValidation ?? null;
    const responseRestoredRevision = {
      markerExposed: Object.hasOwn(responseSummary ?? {}, "restoredRevisionId"),
      markerValue: responseSummary?.restoredRevisionId ?? null,
    };
    const afterRestoredRevision = restoredProvenanceObservation(
      afterRaw.summary,
      after,
    );
    const afterCurrentRevisionIds = afterRestoredRevision.currentRevisionIds;
    const addedRevisionIsOnlyCurrent =
      addedRevisionId !== null &&
      afterCurrentRevisionIds.length === 1 &&
      afterCurrentRevisionIds[0] === addedRevisionId &&
      addedRevision?.isCurrent === true;
    const oldRestoredRevisionNoLongerCurrent = config.expectedRestoredRevisionId
      ? afterRestoredRevision.expectedRevision?.revisionId ===
          config.expectedRestoredRevisionId &&
        afterRestoredRevision.expectedRevision?.isCurrent === false
      : true;
    const expectedBeforeProvenanceMatches = config.expectedRestoredRevisionId
      ? provenanceTransition.before.expectedRestoredRevisionMatchesCurrent === true &&
        provenanceTransition.confirmation
          ?.expectedRestoredRevisionMatchesCurrent === true
      : true;
    provenanceTransition = {
      ...provenanceTransition,
      response: responseRestoredRevision,
      after: afterRestoredRevision,
      addedRevisionId,
      addedRevisionIsOnlyCurrent,
      oldRestoredRevisionNoLongerCurrent,
      expectedTransitionSatisfied:
        expectedBeforeProvenanceMatches &&
        provenanceTransition.before.getterMarkerExposed === false &&
        provenanceTransition.confirmation?.getterMarkerExposed === false &&
        provenanceTransition.payload?.propertyPresent === false &&
        provenanceTransition.payload?.value === null &&
        responseRestoredRevision.markerExposed === false &&
        responseRestoredRevision.markerValue === null &&
        afterRestoredRevision.getterMarkerExposed === false &&
        afterRestoredRevision.getterMarkerValue === null &&
        addedRevisionIsOnlyCurrent &&
        oldRestoredRevisionNoLongerCurrent,
    };
    const allWriteCalls = bridgeCalls.filter(
      (item) => item.command === config.oneTimeWriteCommand,
    );
    const checks = {
      runnerSavePathExecutedExactlyOnce:
        runnerSaveAttempts === 1 &&
        bridgeSaveAttempts === 1 &&
        allWriteCalls.length === 1,
      runnerAndBridgePayloadHashesMatch:
        runnerSavePayloadSha256 === savePayloadSha256 &&
        bridgeSavePayloadSha256 === savePayloadSha256,
      savePayloadObjectIdentityVerified,
      noRunnerCommandBlocked: blockedCalls.length === 0,
      markdownByteIdentityPreserved:
        after.summary.markdownSha256 === confirmation.summary.markdownSha256 &&
        after.summary.markdownLength === confirmation.summary.markdownLength &&
        after.summary.markdownSha256 === config.expectedMarkdownSha256,
      canonicalSummaryJsonAbsent:
        after.summary.summaryJsonPresent === false &&
        !Object.hasOwn(responseSummary ?? {}, "summary_json"),
      restoredRevisionProvenanceTransitionCompleted:
        provenanceTransition.expectedTransitionSatisfied === true,
      transcriptProjectionUnchanged:
        after.transcripts.projectionSha256 === confirmation.transcripts.projectionSha256 &&
        after.transcripts.concatenatedTextSha256 === config.expectedTranscriptSha256,
      generationHistoryUnchanged:
        after.generationHistory.sha256 === confirmation.generationHistory.sha256 &&
        after.generationHistory.count === config.expectedHistoryCount,
      exactlyOneManualRevisionAdded:
        after.manualRevisions.count === confirmation.manualRevisions.count + 1 &&
        addedRevisionIds.length === 1 &&
        Boolean(addedRevision),
      priorManualRevisionsImmutable:
        stableStringify(manualImmutableProjection(oldAfterEntries)) ===
        stableStringify(
          manualImmutableProjection(confirmation.manualRevisions.entries),
        ),
      addedRevisionCurrent: addedRevision?.isCurrent === true,
      addedRevisionIsOnlyCurrent,
      expectedRestoredRevisionNoLongerCurrent:
        oldRestoredRevisionNoLongerCurrent,
      addedRevisionMarkdownMatchesCurrent:
        addedRevision?.markdownSha256 === after.summary.markdownSha256,
      addedRevisionStoredFactMatchesNativeAndDynamic:
        addedRevision?.storedFactValidationSha256 ===
          responseFactValidationSha256 &&
        responseFactValidationSha256 === responseSummaryFactValidationSha256 &&
        responseFactValidationSha256 === after.summary.factValidationSha256,
      addedRevisionSourceGenerationPreserved:
        addedRevision?.sourceGenerationId === config.expectedSourceGenerationId &&
        after.summary.templateSnapshot?.generationId ===
          config.expectedSourceGenerationId,
      templateSnapshotPreserved:
        after.summary.templateSnapshotSha256 ===
        confirmation.summary.templateSnapshotSha256,
      factValidationRecomputedAndStable:
        factValidationConsistent(afterFactValidation) &&
        after.summary.factValidationSha256 ===
          confirmation.summary.factValidationSha256 &&
        afterFactValidation?.aliasesNormalized === false,
      currentContextHashesPreserved:
        stableStringify(after.summary.factContext) ===
          stableStringify(confirmation.summary.factContext),
      nativeResponseIsCanonical:
        responseMarkdownSha256 === after.summary.markdownSha256 &&
        responseFactValidationSha256 === after.summary.factValidationSha256,
      meetingUpdatedAtCaptured:
        typeof after.meeting.updatedAt === "string" && after.meeting.updatedAt.length > 0,
      meetingUpdatedAtAdvanced:
        after.meeting.updatedAt !== confirmation.meeting.updatedAt,
      recordingRemainedIdle: recordingIdle(afterRaw.recording),
      retranscriptionRemainedIdle: afterRaw.retranscription === false,
      exactProductionRouteUnchanged:
        location.origin === "http://tauri.localhost" &&
        location.pathname === "/meeting-details" &&
        new URL(location.href).searchParams.get("id") === config.meetingId,
      commitConfirmed:
        Boolean(saveResponse) &&
        addedRevisionIds.length === 1 &&
        Boolean(addedRevision) &&
        addedRevision?.isCurrent === true,
    };
    const overallPass = Object.values(checks).every(Boolean);
    commitState = overallPass ? "confirmed" : "uncertain";
    return {
      schemaVersion: 2,
      script: "cdp-uat06-revalidation-save",
      capturedAt: new Date().toISOString(),
      meetingId: config.meetingId,
      href: location.href,
      commitState,
      commitStateMeaning:
        "confirmed requires a native response, one uniquely added current revision, a readable after-state, and all post-save checks; uncertain means the attempted save must never be silently retried",
      frozenBaseline: {
        markdownSha256: config.expectedMarkdownSha256,
        transcriptSha256: config.expectedTranscriptSha256,
        sourceGenerationId: config.expectedSourceGenerationId,
        restoredRevisionId: config.expectedRestoredRevisionId || null,
        historyCount: config.expectedHistoryCount,
        manualRevisionCount: config.expectedRevisionCount,
        sha256: frozenBaselineSha256,
      },
      payloadEvidence: {
        candidatePayloadSha256,
        savePayloadSha256,
        runnerSavePayloadSha256,
        bridgeSavePayloadSha256,
        savePayloadObjectIdentityVerified,
        payloadsIdentical: candidatePayloadSha256 === savePayloadSha256,
      },
      safetyPolicy: {
        allowedReadOnlyCommands: config.allowedReadOnlyCommands,
        oneTimeWriteCommand: config.oneTimeWriteCommand,
        knownForbiddenCommands: config.knownForbiddenCommands,
        runnerAllowlistEnforced: true,
        globalBridgeIntercepted: false,
        bridgeCallsOutsideRunnerObservable: false,
        noAutomaticRetry: true,
      },
      preflight,
      stabilityGate,
      provenanceTransition,
      before: first,
      confirmation,
      save: {
        runnerSaveExecuted:
          runnerSaveAttempts === 1 && bridgeSaveAttempts === 1,
        // Compatibility for an older schema-2 reopen runner. New consumers
        // should prefer runnerSaveExecuted, whose scope is explicit.
        saveExecuted: runnerSaveAttempts === 1 && bridgeSaveAttempts === 1,
        responseMessage: saveResponse?.message ?? null,
        responseMarkdownSha256,
        responseFactValidation,
        responseFactValidationSha256,
        responseSummaryFactValidationSha256,
        addedRevisionId,
        addedRevision,
      },
      after,
      commandTrace: {
        runnerCalls,
        bridgeCalls,
        blockedCalls,
        bridgeTraceScope: "calls made by this audit runner only",
        bridgeSaveAttempts,
        runnerSaveAttempts,
      },
      verdict: { ...checks, overallPass },
    };
  } catch (error) {
    return {
      schemaVersion: 2,
      script: "cdp-uat06-revalidation-save",
      capturedAt: new Date().toISOString(),
      meetingId: config.meetingId,
      href: location.href,
      commitState,
      error: {
        name: error instanceof Error ? error.name : "Error",
        message: error instanceof Error ? error.message : String(error),
      },
      frozenBaselineSha256,
      candidatePayloadSha256,
      savePayloadSha256,
      runnerSavePayloadSha256,
      bridgeSavePayloadSha256,
      savePayloadObjectIdentityVerified,
      preflight,
      stabilityGate,
      provenanceTransition,
      before: first,
      confirmation,
      safetyPolicy: {
        allowedReadOnlyCommands: config.allowedReadOnlyCommands,
        oneTimeWriteCommand: config.oneTimeWriteCommand,
        knownForbiddenCommands: config.knownForbiddenCommands,
        runnerAllowlistEnforced: true,
        globalBridgeIntercepted: false,
        bridgeCallsOutsideRunnerObservable: false,
        noAutomaticRetry: true,
      },
      commandTrace: {
        runnerCalls,
        bridgeCalls,
        blockedCalls,
        bridgeTraceScope: "calls made by this audit runner only",
        bridgeSaveAttempts,
        runnerSaveAttempts,
      },
      verdict: { overallPass: false },
    };
  }
}

const cdpPort = process.env.CDP_PORT ?? "9233";
const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`).then((response) =>
  response.json(),
);
const page = targets.find(
  (target) => target.type === "page" && target.url.startsWith("http://tauri.localhost"),
);
if (!page) throw new Error("Meetily production WebView2 debug target was not found");

const socket = new WebSocket(page.webSocketDebuggerUrl);
let nextId = 1;
const pending = new Map();
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  if (!message.id || !pending.has(message.id)) return;
  const { resolve, reject } = pending.get(message.id);
  pending.delete(message.id);
  if (message.error) reject(new Error(JSON.stringify(message.error)));
  else resolve(message.result);
});
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});
function call(method, params = {}) {
  const id = nextId++;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

await call("Runtime.enable");
await call("Page.enable");
await call("Emulation.setDeviceMetricsOverride", {
  width: 1600,
  height: 1000,
  deviceScaleFactor: 1,
  mobile: false,
});
const config = {
  meetingId,
  expectedMarkdownSha256,
  expectedTranscriptSha256,
  expectedSourceGenerationId,
  expectedRestoredRevisionId,
  expectedHistoryCount,
  expectedRevisionCount,
  allowedReadOnlyCommands,
  oneTimeWriteCommand,
  knownForbiddenCommands,
};
const expression = `(${runSaveAudit.toString()})(${JSON.stringify(config)})`;

let result;
try {
  const evaluated = await call("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
    userGesture: true,
  });
  if (evaluated.exceptionDetails) {
    throw new Error(
      evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text,
    );
  }
  result = evaluated.result.value;
} catch (error) {
  // Runtime transport loss after evaluation begins can hide whether the native
  // transaction committed. Preserve that uncertainty and never retry here.
  result = {
    schemaVersion: 2,
    script: "cdp-uat06-revalidation-save",
    capturedAt: new Date().toISOString(),
    meetingId,
    commitState: "uncertain",
    error: {
      name: error instanceof Error ? error.name : "Error",
      message: error instanceof Error ? error.message : String(error),
    },
    commandTrace: null,
    verdict: { overallPass: false },
  };
}

if (screenshotPath) {
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
socket.close();

const output = `${JSON.stringify(result, null, 2)}\n`;
fs.mkdirSync(path.dirname(path.resolve(outputPath)), { recursive: true });
fs.writeFileSync(outputPath, output, { encoding: "utf8", flag: "wx" });
console.log(output.trimEnd());
if (result?.verdict?.overallPass !== true) {
  throw new Error(`UAT-06 revalidation save failed; evidence written to ${outputPath}`);
}
