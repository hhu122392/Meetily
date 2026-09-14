import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const [action, statePath, outputPath, requestPath] = process.argv.slice(2);
if (!action || !statePath || !outputPath) {
  throw new Error(
    "Usage: node moss-functional-ft-cdp.mjs <action> <state.json> <output.json> [request.json]",
  );
}

const allowedActions = new Set([
  "probe",
  "bootstrap",
  "import-audio",
  "recording-start",
  "recording-monitor",
  "recording-pause-resume",
  "recording-stop",
  "meeting-finalize",
  "meeting-snapshot",
  "manual-edit",
  "persistence-verify",
  "moss-cancel",
  "moss-complete",
  "inference-exclusion",
  "whisper-enhance",
  "moss-review-q00",
  "moss-review-strict",
  "moss-workspace-verify",
  "moss-activate",
  "moss-rollback",
  "moss-reactivate",
  "summary-generate",
  "recording-preferences-set",
  "recording-preferences-invalid",
  "fault-snapshot",
]);
if (!allowedActions.has(action)) throw new Error(`Unsupported action: ${action}`);

const readJson = (file, label) => {
  const value = JSON.parse(fs.readFileSync(file, "utf8"));
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be a JSON object`);
  }
  return value;
};
const state = readJson(statePath, "state");
const request = requestPath ? readJson(requestPath, "request") : {};
if (state.schema_version !== 1 || state.stage !== "MOSS_FUNCTIONAL_FT_STATE") {
  throw new Error("State schema/stage is invalid");
}
if (!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(state.run_id ?? "")) {
  throw new Error("State run_id is invalid");
}
if (!/^[0-9a-f]{40}$/.test(state.source_commit ?? "")) {
  throw new Error("State source_commit is invalid");
}
if (!/^[0-9A-F]{64}$/.test(state.candidate_sha256 ?? "")) {
  throw new Error("State candidate_sha256 is invalid");
}
if (fs.existsSync(outputPath)) throw new Error(`Refusing to overwrite output: ${outputPath}`);

const cdpPort = process.env.CDP_PORT;
const cdpTargetId = process.env.CDP_TARGET_ID;
if (!/^\d{1,5}$/.test(cdpPort ?? "") || Number(cdpPort) > 65535 || !cdpTargetId) {
  throw new Error("CDP_PORT and the exact process-bound CDP_TARGET_ID are required");
}

const response = await fetch(`http://127.0.0.1:${cdpPort}/json/list`, {
  signal: AbortSignal.timeout(5000),
});
if (!response.ok) throw new Error(`CDP target list returned HTTP ${response.status}`);
const targets = await response.json();
const page = targets.find((target) => target.id === cdpTargetId && target.type === "page");
if (!page) throw new Error("The exact process-bound WebView2 target was not found");
if (!(page.url.startsWith("http://tauri.localhost") || page.url.startsWith("http://localhost:"))) {
  throw new Error(`The process-bound target has an unexpected URL: ${page.url}`);
}

const socket = new WebSocket(page.webSocketDebuggerUrl);
let nextId = 1;
const pending = new Map();
const failPending = (error) => {
  for (const item of pending.values()) {
    clearTimeout(item.timer);
    item.reject(error);
  }
  pending.clear();
};
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  const item = pending.get(message.id);
  if (!item) return;
  clearTimeout(item.timer);
  pending.delete(message.id);
  if (message.error) item.reject(new Error(JSON.stringify(message.error)));
  else item.resolve(message.result);
});
socket.addEventListener("close", () => failPending(new Error("CDP socket closed")));
await new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error("Timed out opening CDP socket")), 5000);
  socket.addEventListener("open", () => { clearTimeout(timer); resolve(); }, { once: true });
  socket.addEventListener("error", (error) => { clearTimeout(timer); reject(error); }, { once: true });
});

function call(method, params = {}, timeoutMs = 120000) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`CDP ${method} timed out after ${timeoutMs}ms`));
    }, timeoutMs);
    pending.set(id, { resolve, reject, timer });
    socket.send(JSON.stringify({ id, method, params }));
  });
}

async function evaluate(expression, timeoutMs = 120000) {
  const evaluated = await call("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
    userGesture: true,
  }, timeoutMs);
  if (evaluated.exceptionDetails) {
    throw new Error(
      evaluated.exceptionDetails.exception?.description
      ?? evaluated.exceptionDetails.text
      ?? "Runtime.evaluate failed",
    );
  }
  return evaluated.result.value;
}

async function invokeRaw(command, payload = {}) {
  return evaluate(`(async () => {
    try {
      const value = await window.__TAURI_INTERNALS__.invoke(
        ${JSON.stringify(command)}, ${JSON.stringify(payload)}
      );
      return { ok: true, value };
    } catch (error) {
      const value = error && typeof error === 'object' ? error : null;
      return { ok: false, value, error: typeof error === 'string' ? error : JSON.stringify(error) };
    }
  })()`);
}

async function invoke(command, payload = {}) {
  const result = await invokeRaw(command, payload);
  if (!result.ok) {
    const error = new Error(`${command} failed: ${result.error}`);
    error.tauri = result.value;
    throw error;
  }
  return result.value;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const systemMonotonicMs = () => Number(process.hrtime.bigint()) / 1_000_000;
const PERFORMANCE_CLOCK = "node_performance_now";
const PERFORMANCE_GATES = Object.freeze({
  stop_feedback: 1000,
  page_unlock: 10000,
  enhance_feedback: 1000,
  cancel_feedback: 1000,
  summary_completion: 180000,
  whisper_enhancement_completion: 300000,
});
const performanceSample = (startedMonotonicMs, completedMonotonicMs, thresholdMs) => ({
  clock: PERFORMANCE_CLOCK,
  started_monotonic_ms: startedMonotonicMs,
  completed_monotonic_ms: completedMonotonicMs,
  elapsed_ms: completedMonotonicMs - startedMonotonicMs,
  threshold_ms: thresholdMs,
  within_threshold: completedMonotonicMs - startedMonotonicMs <= thresholdMs,
});
const sha256Text = (value) => crypto.createHash("sha256").update(value, "utf8").digest("hex").toUpperCase();
const MOSS_DECODE_PARAMETERS_JSON = '{"language":"zh","timestamps":"segment","diarize":"on"}';
const nativeDecodeContract = (run) => {
  const apiFields = {
    languageRequested: run?.languageRequested,
    languageResolved: run?.languageResolved,
    decodeParametersJson: run?.decodeParametersJson,
    decodeParametersSha256: run?.decodeParametersSha256,
  };
  const missing = Object.entries(apiFields)
    .filter(([, value]) => typeof value !== "string" || value.trim() === "")
    .map(([name]) => name);
  if (missing.length > 0) throw new Error(`MOSS native decode proof is missing: ${missing.join(",")}`);
  if (apiFields.languageRequested !== "zh-CN") {
    throw new Error(`MOSS requested language is not explicit zh-CN: ${apiFields.languageRequested}`);
  }
  if (apiFields.languageResolved !== "zh-CN") {
    throw new Error(`MOSS native resolved language is not zh-CN: ${apiFields.languageResolved}`);
  }
  if (apiFields.decodeParametersJson !== MOSS_DECODE_PARAMETERS_JSON) {
    throw new Error(`MOSS native decode parameters are not the frozen zh-CN contract: ${apiFields.decodeParametersJson}`);
  }
  const recomputed = sha256Text(apiFields.decodeParametersJson);
  if (recomputed !== apiFields.decodeParametersSha256.toUpperCase()) {
    throw new Error(`MOSS native decode parameter hash mismatch: ${apiFields.decodeParametersSha256}`);
  }
  return {
    source: "api_moss_get_workspace.runs",
    language_requested: apiFields.languageRequested,
    language_resolved: apiFields.languageResolved,
    decode_parameters_json: apiFields.decodeParametersJson,
    decode_parameters_sha256: apiFields.decodeParametersSha256,
    recomputed_decode_parameters_sha256: recomputed,
    api_fields: apiFields,
    passed: true,
  };
};
const transcriptText = (meeting) => (meeting?.transcripts ?? []).map((item) => item.text ?? "").join("\n");
const normalizedTimes = (segments) => (segments ?? []).map((item) => ({
  start: item.audio_start_time ?? item.audioStartTime ?? item.start_time ?? item.startTime
    ?? item.start_seconds ?? item.startSeconds,
  end: item.audio_end_time ?? item.audioEndTime ?? item.end_time ?? item.endTime
    ?? item.end_seconds ?? item.endSeconds,
}));
const timeAudit = (segments, duration = null) => {
  const rows = normalizedTimes(segments);
  const finite = rows.every((item) => Number.isFinite(item.start) && Number.isFinite(item.end));
  const legal = finite && rows.every((item) => item.start >= 0 && item.end > item.start
    && (duration === null || item.end <= duration + 0.001));
  const monotonic = legal && rows.every((item, index) => index === 0
    || (item.start >= rows[index - 1].start && item.end >= rows[index - 1].end));
  const lastEnd = rows.length > 0 ? rows.at(-1).end : null;
  return { count: rows.length, finite, legal, monotonic, lastEnd };
};
const saveState = () => {
  const temporary = `${statePath}.${process.pid}.${crypto.randomUUID()}.tmp`;
  fs.writeFileSync(temporary, `${JSON.stringify(state, null, 2)}\n`, { encoding: "utf8", flag: "wx" });
  fs.renameSync(temporary, statePath);
};
const requireMeetingId = () => {
  const meetingId = request.meeting_id ?? state.meeting_id;
  if (typeof meetingId !== "string" || !meetingId) throw new Error("A meeting_id is required");
  return meetingId;
};
const workspace = (runId = state.moss_run_id ?? null) => invoke("api_moss_get_workspace", {
  request: { meetingId: requireMeetingId(), selectedRunId: runId },
});

async function navigate(url) {
  await call("Page.enable");
  await call("Page.navigate", { url });
  await sleep(2200);
}

async function prepareMossUi(meetingId) {
  await navigate(`http://tauri.localhost/meeting-details?id=${encodeURIComponent(meetingId)}&source=recording`);
  return evaluate(`(async () => {
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const visible = (element) => {
      const rectangle = element.getBoundingClientRect();
      return rectangle.width > 0 && rectangle.height > 0;
    };
    const openerLabels = [
      '重新转写以增强录制的音频',
      'Retranscribe to enhance your recorded audio',
      '增强',
      'Enhance',
    ];
    const openDeadline = performance.now() + 30000;
    let openControls = [];
    while (performance.now() <= openDeadline) {
      openControls = [...document.querySelectorAll('button')].filter((button) => {
        const label = button.getAttribute('aria-label')?.trim() ?? '';
        const title = button.getAttribute('title')?.trim() ?? '';
        const text = button.textContent?.trim() ?? '';
        return visible(button) && !button.disabled
          && openerLabels.some((wanted) => label === wanted || title === wanted || text === wanted);
      });
      if (openControls.length === 1) break;
      await sleep(50);
    }
    if (openControls.length !== 1) {
      throw new Error('Expected exactly one visible recording enhancement control, got ' + openControls.length);
    }
    openControls[0].click();

    const dialogDeadline = performance.now() + 30000;
    let mossTabs = [];
    let mossStartControls = [];
    while (performance.now() <= dialogDeadline) {
      mossTabs = [...document.querySelectorAll('[role="tab"]')].filter((tab) =>
        visible(tab) && (tab.textContent?.includes('MOSS') ?? false));
      if (mossTabs.length > 1) throw new Error('Multiple visible MOSS mode tabs were found');
      if (mossTabs.length === 1 && mossTabs[0].getAttribute('data-state') !== 'active') mossTabs[0].click();
      mossStartControls = [...document.querySelectorAll('button')].filter((button) => {
        const text = button.textContent?.trim() ?? '';
        return visible(button) && !button.disabled
          && (text === '开始 MOSS 增强' || text === 'Start MOSS enhancement');
      });
      if (mossStartControls.length === 1) break;
      await sleep(50);
    }
    if (mossStartControls.length !== 1) {
      throw new Error('Expected exactly one enabled MOSS start control, got ' + mossStartControls.length);
    }
    return { enhancementControlCount: openControls.length, mossTabCount: mossTabs.length,
      mossStartControlCount: mossStartControls.length };
  })()`, 70000);
}

async function startMossFromVisibleUi(meetingId) {
  const before = await workspace();
  const priorRunIds = new Set((before.runs ?? []).map((run) => run.runId));
  const prepared = await prepareMossUi(meetingId);
  const startedMonotonicMs = performance.now();
  const ui = await evaluate(`(async () => {
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const visible = (element) => {
      const rectangle = element.getBoundingClientRect();
      return rectangle.width > 0 && rectangle.height > 0;
    };
    const mossStartControls = [...document.querySelectorAll('button')].filter((button) => {
      const text = button.textContent?.trim() ?? '';
      return visible(button) && !button.disabled
        && (text === '开始 MOSS 增强' || text === 'Start MOSS enhancement');
    });
    if (mossStartControls.length !== 1) {
      throw new Error('Expected exactly one enabled MOSS start control, got ' + mossStartControls.length);
    }
    mossStartControls[0].click();
    const deadline = performance.now() + ${JSON.stringify(PERFORMANCE_GATES.enhance_feedback)};
    let uiProcessingVisible = false;
    let feedbackText = null;
    while (performance.now() <= deadline) {
      const statuses = [...document.querySelectorAll('[role="status"]')]
        .filter(visible)
        .map((element) => element.textContent?.trim() ?? '')
        .filter(Boolean);
      feedbackText = statuses.find((value) =>
        value.includes('准备中') || value.includes('运行中') || value.includes('正在加载 MOSS')
        || value.includes('Preparing') || value.includes('Running') || value.includes('Loading MOSS')) ?? null;
      if (feedbackText) {
        uiProcessingVisible = true;
        break;
      }
      await sleep(20);
    }
    return { mossStartControlCount: mossStartControls.length, uiProcessingVisible, feedbackText };
  })()`, PERFORMANCE_GATES.enhance_feedback + 10000);
  const completedMonotonicMs = performance.now();
  const after = await workspace();
  const run = (after.runs ?? []).find((candidate) =>
    !priorRunIds.has(candidate.runId) && ['preparing', 'running'].includes(candidate.state)) ?? null;
  const timing = performanceSample(startedMonotonicMs, completedMonotonicMs,
    PERFORMANCE_GATES.enhance_feedback);
  timing.within_threshold &&= ui.uiProcessingVisible === true;
  return { before, after, run, prepared, ui, performance: timing };
}

async function cancelMossFromVisibleUi() {
  const startedMonotonicMs = performance.now();
  const ui = await evaluate(`(async () => {
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const visible = (element) => {
      const rectangle = element.getBoundingClientRect();
      return rectangle.width > 0 && rectangle.height > 0;
    };
    const mossCancelControls = [...document.querySelectorAll('button')].filter((button) => {
      const text = button.textContent?.trim() ?? '';
      return visible(button) && !button.disabled
        && (text === '取消 MOSS 任务' || text === 'Cancel MOSS run');
    });
    if (mossCancelControls.length !== 1) {
      throw new Error('Expected exactly one enabled MOSS cancel control, got ' + mossCancelControls.length);
    }
    mossCancelControls[0].click();
    const deadline = performance.now() + ${JSON.stringify(PERFORMANCE_GATES.cancel_feedback)};
    let uiCancellationVisible = false;
    let pageUsableAfterCancellation = false;
    let feedbackText = null;
    while (performance.now() <= deadline) {
      const body = document.body?.innerText ?? '';
      for (const wanted of ['正在请求取消', '已取消', 'Cancellation requested', 'Cancelled']) {
        if (body.includes(wanted)) {
          feedbackText = wanted;
          uiCancellationVisible = true;
          break;
        }
      }
      const workspaceRoot = document.querySelector('section[aria-labelledby="moss-review-heading"]');
      pageUsableAfterCancellation = workspaceRoot?.getAttribute('aria-busy') === 'false'
        && [...workspaceRoot.querySelectorAll('button')].some((button) => {
        const text = button.textContent?.trim() ?? '';
        return visible(button) && !button.disabled
          && (text === '刷新' || text === 'Refresh');
        });
      if (uiCancellationVisible && pageUsableAfterCancellation) break;
      await sleep(20);
    }
    return { mossCancelControlCount: mossCancelControls.length, uiCancellationVisible,
      pageUsableAfterCancellation, feedbackText };
  })()`, PERFORMANCE_GATES.cancel_feedback + 10000);
  const completedMonotonicMs = performance.now();
  const timing = performanceSample(startedMonotonicMs, completedMonotonicMs,
    PERFORMANCE_GATES.cancel_feedback);
  timing.within_threshold &&= ui.uiCancellationVisible === true
    && ui.pageUsableAfterCancellation === true;
  return { ui, performance: timing };
}

async function pollMoss(runId, timeoutMs, terminal = new Set(["completed", "failed", "cancelled"])) {
  const started = performance.now();
  const samples = [];
  let latest = null;
  while (performance.now() - started < timeoutMs) {
    const current = await workspace(runId);
    const run = current.runs?.find((item) => item.runId === runId) ?? null;
    const sample = {
      at: new Date().toISOString(),
      state: run?.state ?? null,
      stage: run?.progress?.stage ?? null,
      percentage: run?.progress?.percentage ?? null,
      candidateRevision: run?.candidateRevision ?? null,
      errorCode: run?.errorCode ?? null,
    };
    if (samples.length === 0 || JSON.stringify(samples.at(-1)) !== JSON.stringify(sample)) samples.push(sample);
    latest = { current, run };
    if (run && terminal.has(run.state)) return { ...latest, samples, elapsed_ms: performance.now() - started };
    await sleep(1000);
  }
  throw new Error(`MOSS run ${runId} did not reach a terminal state in ${timeoutMs}ms`);
}

async function generateSummary(meetingId, timeoutMs) {
  await navigate(`http://tauri.localhost/meeting-details?id=${encodeURIComponent(meetingId)}&source=recording`);
  const nodeStartedMonotonicMs = performance.now();
  const result = await evaluate(`(async () => {
    const meetingId = ${JSON.stringify(meetingId)};
    const requestedTimeoutMs = ${JSON.stringify(timeoutMs)};
    const thresholdMs = ${JSON.stringify(PERFORMANCE_GATES.summary_completion)};
    const clock = ${JSON.stringify(PERFORMANCE_CLOCK)};
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const historyBefore = await invoke('api_list_summary_generation_history', { meetingId });
    const meetingBefore = await invoke('api_get_meeting', { meetingId });
    const visible = (element) => { const r = element.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
    const buttons = [...document.querySelectorAll('button')].filter((button) => {
      const text = button.innerText.trim();
      return visible(button) && !button.disabled && (
        text === '生成摘要' || text === '重新生成摘要'
        || text === 'Generate Summary' || text === 'Regenerate Summary'
        || button.title === '生成 AI 摘要' || button.title === '重新生成 AI 摘要'
        || button.title === 'Generate AI Summary' || button.title === 'Regenerate AI Summary'
      );
    });
    if (buttons.length !== 1) throw new Error('Expected exactly one enabled summary action, got ' + buttons.length);
    const startedAt = new Date().toISOString();
    const started = performance.now();
    buttons[0].click();
    let confirmationCount = 0;
    const confirmationDeadline = performance.now() + 4000;
    while (performance.now() < confirmationDeadline) {
      const confirmations = [...document.querySelectorAll('button')].filter((button) =>
        visible(button) && !button.disabled
        && (button.innerText.includes('使用当前最新模板')
          || button.innerText.includes('Use latest selected template')));
      if (confirmations.length > 0) {
        if (confirmations.length !== 1) throw new Error('Multiple template confirmation buttons');
        confirmationCount = 1;
        confirmations[0].click();
        break;
      }
      await sleep(50);
    }
    const observations = [];
    let terminal = null;
    let thresholdExceeded = false;
    const hardLimitMs = Math.min(requestedTimeoutMs, thresholdMs);
    while (performance.now() - started <= hardLimitMs) {
      const history = await invoke('api_list_summary_generation_history', { meetingId });
      const newest = history[0] ?? null;
      const body = document.body?.innerText ?? '';
      const running = newest?.status === 'pending' || newest?.status === 'processing'
        || body.includes('正在生成 AI 摘要') || body.includes('正在重新生成摘要')
        || body.includes('Generating AI Summary') || body.includes('Regenerating summary');
      observations.push({ at: new Date().toISOString(), historyCount: history.length,
        newestStatus: newest?.status ?? null, running });
      if (history.length === historyBefore.length + 1 && newest?.status === 'completed' && !running) {
        terminal = { history, newest };
        break;
      }
      if (history.length > historyBefore.length && newest?.status === 'failed') {
        terminal = { history, newest };
        break;
      }
      await sleep(500);
    }
    const completedMonotonicMs = performance.now();
    if (!terminal) {
      thresholdExceeded = completedMonotonicMs - started >= thresholdMs;
      try { await invoke('api_cancel_summary', { meetingId }); } catch (_) { /* evidence below records failure */ }
    }
    const historyAfter = await invoke('api_list_summary_generation_history', { meetingId });
    const meetingAfter = await invoke('api_get_meeting', { meetingId });
    const newest = historyAfter[0] ?? null;
    const source = newest?.summarySourceBinding ?? newest?.summary_source_binding
      ?? meetingAfter?.summary_source_binding ?? meetingAfter?.summarySourceBinding ?? null;
    return {
      startedAt, finishedAt: new Date().toISOString(), elapsedMs: completedMonotonicMs - started,
      confirmationCount, historyBefore, historyAfter, meetingBefore, meetingAfter, newest,
      sourceBinding: source, observations, terminal,
      performance: {
        summary_completion: {
          clock,
          started_monotonic_ms: started,
          completed_monotonic_ms: completedMonotonicMs,
          elapsed_ms: completedMonotonicMs - started,
          threshold_ms: thresholdMs,
          within_threshold: Boolean(terminal) && completedMonotonicMs - started <= thresholdMs,
          threshold_exceeded: thresholdExceeded,
        },
      },
      verdict: {
        completed: historyAfter.length === historyBefore.length + 1 && newest?.status === 'completed',
        exactlyOneNewHistory: historyAfter.length === historyBefore.length + 1,
        nonEmptySummary: Boolean(meetingAfter?.summary) && JSON.stringify(meetingAfter.summary).length > 20,
        sourceBindingPresent: Boolean(source),
      },
    };
  })()`, timeoutMs + 30000);
  const nodeCompletedMonotonicMs = performance.now();
  result.performance.summary_completion = performanceSample(nodeStartedMonotonicMs,
    nodeCompletedMonotonicMs, PERFORMANCE_GATES.summary_completion);
  result.performance.summary_completion.within_threshold &&= result.verdict.completed === true;
  result.performance.summary_completion.threshold_exceeded =
    nodeCompletedMonotonicMs - nodeStartedMonotonicMs > PERFORMANCE_GATES.summary_completion;
  result.action_completed_system_monotonic_ms = systemMonotonicMs();
  return result;
}

await call("Runtime.enable");
let report;

try {
  if (action === "probe") {
    const sessionId = await invoke("get_app_session_id", {});
    const storage = await invoke("get_storage_status", {});
    report = { session_id: sessionId, storage, href: page.url, ready: true };
  } else if (action === "bootstrap") {
    const whisperModel = request.whisper_model;
    const qwenModel = request.qwen_model;
    const recordingRoot = request.recording_root;
    if (![whisperModel, qwenModel, recordingRoot].every((item) => typeof item === "string" && item)) {
      throw new Error("bootstrap requires whisper_model, qwen_model and recording_root");
    }
    const browser = await evaluate(`(() => {
      const features = { importAndRetranscribe: true, moss_post_meeting_enhancement: true };
      localStorage.setItem('betaFeatures', JSON.stringify(features));
      localStorage.setItem('isAutoSummary', 'false');
      return { features };
    })()`);
    await call("Page.reload", { ignoreCache: true });
    await sleep(2500);
    await invoke("api_save_transcript_config", { provider: "localWhisper", model: whisperModel, apiKey: null });
    await invoke("set_recording_preferences", { preferences: {
      save_folder: recordingRoot, auto_save: true, file_format: "wav",
      preferred_mic_device: null, preferred_system_device: null,
    } });
    await invoke("whisper_load_model", { modelName: whisperModel });
    await invoke("api_save_model_config", {
      provider: "builtin-ai", model: qwenModel, whisperModel, apiKey: null, ollamaEndpoint: null,
    });
    const [transcriptConfig, recordingPreferences, whisperLoaded, currentWhisper,
      qwenInfo, modelConfig, mossStatus, storage] = await Promise.all([
      invoke("api_get_transcript_config", {}),
      invoke("get_recording_preferences", {}),
      invoke("whisper_is_model_loaded", {}),
      invoke("whisper_get_current_model", {}),
      invoke("builtin_ai_get_model_info", { modelName: qwenModel }),
      invoke("api_get_model_config", {}),
      invoke("api_moss_get_system_status", {}),
      invoke("get_storage_status", {}),
    ]);
    report = { browser, transcriptConfig, recordingPreferences, whisperLoaded, currentWhisper,
      qwenInfo, modelConfig, mossStatus, storage,
      verdict: {
        whisperConfigured: transcriptConfig?.provider === "localWhisper" && transcriptConfig?.model === whisperModel,
        whisperLoaded: whisperLoaded === true && currentWhisper === whisperModel,
        recordingRootApplied: path.win32.normalize(recordingPreferences?.save_folder ?? "").toLowerCase()
          === path.win32.normalize(recordingRoot).toLowerCase(),
        qwenConfiguredAndAvailable: modelConfig?.provider === "builtin-ai" && modelConfig?.model === qwenModel
          && qwenInfo?.status?.type === "available",
        mossReady: mossStatus?.availability === "ready" && mossStatus?.installed === true
          && mossStatus?.health === "healthy",
      } };
  } else if (action === "import-audio") {
    const audioPath = request.audio_path;
    const title = request.title;
    const language = request.language ?? "zh-CN";
    const model = request.model;
    const provider = request.provider ?? "localWhisper";
    const timeoutMs = Number(request.timeout_ms ?? 3600000);
    if (![audioPath, title, model, provider].every((item) => typeof item === "string" && item.trim())) {
      throw new Error("import-audio requires audio_path, title, model and provider");
    }
    if (!Number.isFinite(timeoutMs) || timeoutMs < 1000 || timeoutMs > 7200000) {
      throw new Error("import-audio timeout_ms is outside the approved range");
    }
    const before = await invoke("api_get_meetings", {});
    const beforeIds = new Set((before ?? []).map((item) => item.id));
    const startedAt = new Date().toISOString();
    const started = performance.now();
    const startResult = await invoke("start_import_audio_command", {
      sourcePath: audioPath,
      title,
      language,
      model,
      provider,
    });
    const progress = [];
    let observedBusy = false;
    while (performance.now() - started < timeoutMs) {
      const busy = await invoke("is_import_in_progress_command", {});
      progress.push({ at: new Date().toISOString(), busy });
      observedBusy ||= busy === true;
      if (busy === false && (observedBusy || performance.now() - started >= 1000)) break;
      await sleep(500);
    }
    const stillBusy = await invoke("is_import_in_progress_command", {});
    if (stillBusy) throw new Error(`Audio import did not finish in ${timeoutMs}ms`);
    const meetings = await invoke("api_get_meetings", {});
    const matches = (meetings ?? []).filter((item) => !beforeIds.has(item.id) && item.title === title);
    if (matches.length !== 1) {
      throw new Error(`Expected one newly imported exact-title meeting, got ${matches.length}`);
    }
    const meeting = await invoke("api_get_meeting", { meetingId: matches[0].id });
    if (!Array.isArray(meeting?.transcripts) || meeting.transcripts.length === 0) {
      throw new Error("Imported meeting has no persisted Whisper transcript");
    }
    const elapsedMs = performance.now() - started;
    state.meeting_id = meeting.id;
    state.meeting_original_title = meeting.title;
    state.meeting_folder = meeting.folder_path ?? meeting.folderPath ?? null;
    if (typeof state.meeting_folder !== "string" || !state.meeting_folder) {
      throw new Error("Imported meeting did not expose its persisted folder path");
    }
    state.meeting_transcript_sha256 = sha256Text(transcriptText(meeting));
    state.import_audio_path = audioPath;
    state.import_completed_at = new Date().toISOString();
    saveState();
    report = {
      startedAt,
      completedAt: state.import_completed_at,
      elapsedMs,
      startResult,
      progress,
      meeting,
      transcript_sha256: state.meeting_transcript_sha256,
      transcript_time_audit: timeAudit(meeting.transcripts, request.expected_duration_seconds ?? null),
      verdict: {
        importFinished: stillBusy === false,
        exactlyOneNewMeeting: true,
        exactTitle: meeting.title === title,
        transcriptPersisted: meeting.transcripts.length > 0,
      },
    };
  } else if (action === "recording-start") {
    await navigate("http://tauri.localhost/");
    report = await evaluate(`(async () => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
      const before = await invoke('get_recording_state', {});
      if (before.is_recording || before.is_active) throw new Error('Recording was already active');
      const visible = (element) => { const r = element.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
      const buttons = [...document.querySelectorAll('button')].filter((button) => {
        const text = button.textContent?.trim() ?? '';
        const label = button.getAttribute('aria-label') ?? '';
        return visible(button) && !button.disabled
          && (text.includes('开始录音') || label.includes('开始录音')
            || text.includes('Start recording') || label.includes('Start Recording'));
      });
      if (buttons.length < 1) throw new Error('No enabled Start Recording control');
      buttons.sort((a, b) => b.getBoundingClientRect().bottom - a.getBoundingClientRect().bottom)[0].click();
      const samples = [];
      const deadline = performance.now() + 15000;
      while (performance.now() < deadline) {
        const current = await invoke('get_recording_state', {});
        samples.push({ at: new Date().toISOString(), current });
        if (current.is_recording && current.is_active) break;
        await sleep(200);
      }
      const after = await invoke('get_recording_state', {});
      return { before, after, samples, folder: await invoke('get_meeting_folder_path', {}),
        meetingName: await invoke('get_current_meeting_name', {}),
        verdict: { oneClick: true, backendStarted: after.is_recording === true && after.is_active === true } };
    })()`);
    state.recording_started_at = new Date().toISOString();
    state.meeting_folder = report.folder;
    state.meeting_name = report.meetingName;
    saveState();
  } else if (action === "recording-monitor") {
    const durationMs = Math.max(1000, Math.min(Number(request.duration_ms), 6 * 60 * 60 * 1000));
    const pollMs = Math.max(100, Math.min(Number(request.poll_ms ?? 500), 5000));
    if (!Number.isFinite(durationMs)) throw new Error("recording-monitor duration_ms is invalid");
    report = await evaluate(`(async () => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
      const durationMs = ${JSON.stringify(durationMs)};
      const pollMs = ${JSON.stringify(pollMs)};
      const started = performance.now();
      const changes = [];
      let fingerprint = null;
      while (performance.now() - started < durationMs) {
        const history = await invoke('get_transcript_history', {});
        const next = JSON.stringify(history);
        if (next !== fingerprint) {
          changes.push({ at: new Date().toISOString(), elapsedMs: performance.now() - started, segments: history });
          fingerprint = next;
        }
        const recording = await invoke('get_recording_state', {});
        if (!recording.is_recording && !recording.is_active) break;
        await sleep(pollMs);
      }
      return { changes, finalHistory: await invoke('get_transcript_history', {}),
        finalState: await invoke('get_recording_state', {}), elapsedMs: performance.now() - started };
    })()`, durationMs + 30000);
    const useful = report.changes.filter((item) => (item.segments ?? []).some((segment) => String(segment.text ?? "").trim()));
    report.metrics = { change_count: report.changes.length, nonempty_change_count: useful.length,
      final_segment_count: Array.isArray(report.finalHistory) ? report.finalHistory.length : 0,
      time_audit: timeAudit(report.finalHistory) };
  } else if (action === "recording-pause-resume") {
    const pauseMs = Math.max(500, Math.min(Number(request.pause_ms ?? 1500), 10000));
    const continuationTimeoutMs = Math.max(1000, Math.min(Number(request.continuation_timeout_ms ?? 30000), 60000));
    report = await evaluate(`(async () => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
      const pauseMs = ${JSON.stringify(pauseMs)};
      const continuationTimeoutMs = ${JSON.stringify(continuationTimeoutMs)};
      const visible = (element) => { const r = element.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
      const findControl = (wanted) => [...document.querySelectorAll('button')].filter((button) => {
        const label = button.getAttribute('aria-label') ?? '';
        const text = button.textContent?.trim() ?? '';
        return visible(button) && !button.disabled && wanted.some((item) => label.includes(item) || text.includes(item));
      });
      const before = await invoke('get_recording_state', {});
      const historyBeforePause = await invoke('get_transcript_history', {});
      const pauseControls = findControl(['暂停录音', 'Pause Recording', 'Pause recording']);
      if (pauseControls.length !== 1) throw new Error('Expected exactly one enabled Pause Recording control, got ' + pauseControls.length);
      pauseControls[0].click();
      const pauseDeadline = performance.now() + 10000;
      let paused = null;
      while (performance.now() <= pauseDeadline) {
        paused = await invoke('get_recording_state', {});
        if (paused.is_paused === true) break;
        await sleep(25);
      }
      const historyAtPause = await invoke('get_transcript_history', {});
      await sleep(pauseMs);
      const historyBeforeResume = await invoke('get_transcript_history', {});
      const resumeControls = findControl(['恢复录音', 'Resume Recording', 'Resume recording']);
      if (resumeControls.length !== 1) throw new Error('Expected exactly one enabled Resume Recording control, got ' + resumeControls.length);
      resumeControls[0].click();
      const resumeDeadline = performance.now() + 10000;
      let resumed = null;
      while (performance.now() <= resumeDeadline) {
        resumed = await invoke('get_recording_state', {});
        if (resumed.is_paused === false && resumed.is_recording === true) break;
        await sleep(25);
      }
      const continuationStarted = performance.now();
      const resumeFingerprint = JSON.stringify(historyBeforeResume);
      let historyAfterResume = await invoke('get_transcript_history', {});
      while (performance.now() - continuationStarted <= continuationTimeoutMs
        && JSON.stringify(historyAfterResume) === resumeFingerprint) {
        await sleep(100);
        historyAfterResume = await invoke('get_transcript_history', {});
      }
      return {
        before, paused, resumed, pause_duration_ms: pauseMs,
        historyBeforePause, historyAtPause, historyBeforeResume, historyAfterResume,
        transcript_change_count_before_resume: historyBeforeResume.length,
        transcript_change_count_after_resume: historyAfterResume.length,
        verdict: {
          pauseObserved: paused?.is_paused === true,
          resumeObserved: resumed?.is_paused === false && resumed?.is_recording === true,
          transcriptContinuesAfterResume: JSON.stringify(historyAfterResume) !== resumeFingerprint
            && historyAfterResume.length > historyBeforeResume.length,
        },
      };
    })()`, pauseMs + continuationTimeoutMs + 30000);
    state.recording_pause_resume = {
      pause_observed: report.verdict.pauseObserved,
      transcript_change_count_before_resume: report.transcript_change_count_before_resume,
      transcript_change_count_after_resume: report.transcript_change_count_after_resume,
      transcript_continues_after_resume: report.verdict.transcriptContinuesAfterResume,
    };
    saveState();
  } else if (action === "recording-stop") {
    const startedMonotonicMs = performance.now();
    const feedbackPhase = await evaluate(`(async () => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
      const stopFeedbackThresholdMs = ${JSON.stringify(PERFORMANCE_GATES.stop_feedback)};
      const before = await invoke('get_recording_state', {});
      const visible = (element) => { const r = element.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
      const buttons = [...document.querySelectorAll('button')].filter((button) => {
        const text = button.textContent?.trim() ?? '';
        const label = button.getAttribute('aria-label') ?? '';
        return visible(button) && !button.disabled
          && (text.includes('停止录音') || label.includes('停止录音')
            || text.includes('Stop recording') || label.includes('Stop Recording'));
      });
      if (buttons.length !== 1) throw new Error('Expected exactly one enabled Stop Recording control, got ' + buttons.length);
      const browserStartedMonotonicMs = performance.now();
      buttons[0].click();
      let feedbackObserved = false;
      const feedbackDeadline = browserStartedMonotonicMs + stopFeedbackThresholdMs;
      while (performance.now() <= feedbackDeadline) {
        const body = document.body?.innerText ?? '';
        if (buttons[0].disabled || !document.contains(buttons[0])
          || body.includes('正在停止') || body.includes('正在处理')
          || body.includes('Stopping') || body.includes('Processing recording')) {
          feedbackObserved = true;
          break;
        }
        await sleep(20);
      }
      return { before, feedbackObserved };
    })()`, PERFORMANCE_GATES.stop_feedback + 10000);
    const feedbackCompletedMonotonicMs = performance.now();
    const remainingUnlockMs = Math.max(0,
      PERFORMANCE_GATES.page_unlock - (feedbackCompletedMonotonicMs - startedMonotonicMs));
    const unlockPhase = await evaluate(`(async () => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
      const visible = (element) => { const r = element.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
      const remainingUnlockMs = ${JSON.stringify(remainingUnlockMs)};
      const unlockDeadline = performance.now() + remainingUnlockMs;
      const samples = [];
      let queue = null;
      let unlocked = false;
      while (performance.now() <= unlockDeadline) {
        const current = await invoke('get_recording_state', {});
        queue = await invoke('get_transcription_status', {});
        const startEnabled = [...document.querySelectorAll('button')].some((button) => {
          const text = button.textContent?.trim() ?? '';
          const label = button.getAttribute('aria-label') ?? '';
          return visible(button) && !button.disabled
            && (text.includes('开始录音') || label.includes('开始录音')
              || text.includes('Start recording') || label.includes('Start Recording'));
        });
        const detailsEnabled = location.href.includes('meeting-details')
          && [...document.querySelectorAll('button')].some((button) => visible(button) && !button.disabled);
        const pageUsable = startEnabled || detailsEnabled;
        samples.push({ at: new Date().toISOString(), monotonic_ms: performance.now(), current, queue, startEnabled, detailsEnabled, pageUsable });
        if (!current.is_recording && !current.is_active && queue?.chunks_in_queue === 0
          && queue?.is_processing === false && pageUsable) {
          unlocked = true;
          break;
        }
        await sleep(50);
      }
      const after = await invoke('get_recording_state', {});
      queue = await invoke('get_transcription_status', {});
      return { after, queue, samples, unlocked };
    })()`, remainingUnlockMs + 10000);
    const unlockCompletedMonotonicMs = performance.now();
    const stopFeedback = performanceSample(startedMonotonicMs, feedbackCompletedMonotonicMs,
      PERFORMANCE_GATES.stop_feedback);
    stopFeedback.within_threshold &&= feedbackPhase.feedbackObserved === true;
    const pageUnlock = performanceSample(startedMonotonicMs, unlockCompletedMonotonicMs,
      PERFORMANCE_GATES.page_unlock);
    pageUnlock.within_threshold &&= unlockPhase.unlocked === true;
    report = {
      before: feedbackPhase.before,
      after: unlockPhase.after,
      queue_at_finalization: unlockPhase.queue,
      samples: unlockPhase.samples,
      action_completed_monotonic_ms: unlockCompletedMonotonicMs,
      performance: { stop_feedback: stopFeedback, page_unlock: pageUnlock },
      verdict: {
        wasRecording: feedbackPhase.before.is_recording === true || feedbackPhase.before.is_active === true,
        backendStopped: unlockPhase.after.is_recording === false && unlockPhase.after.is_active === false,
        queueZeroAtFinalization: unlockPhase.queue?.chunks_in_queue === 0 && unlockPhase.queue?.is_processing === false,
        stopFeedbackWithinThreshold: stopFeedback.within_threshold,
        pageUnlockedWithinThreshold: pageUnlock.within_threshold,
      },
    };
    state.recording_stopped_at = new Date().toISOString();
    state.recording_finalization_queue = report.queue_at_finalization;
    saveState();
  } else if (action === "meeting-finalize") {
    const prefix = String(request.title_prefix ?? "MOSS-FT");
    const started = Date.parse(state.recording_started_at ?? "");
    if (!Number.isFinite(started)) throw new Error("recording_started_at is missing from state");
    let selected = null;
    let meeting = null;
    const deadline = performance.now() + Number(request.timeout_ms ?? 180000);
    while (performance.now() < deadline) {
      const meetings = await invoke("api_get_meetings", {});
      selected = meetings
        .filter((item) => Date.parse(item.created_at ?? item.createdAt ?? 0) >= started - 5000)
        .sort((a, b) => Date.parse(b.created_at ?? b.createdAt ?? 0) - Date.parse(a.created_at ?? a.createdAt ?? 0))[0] ?? null;
      if (selected?.id) {
        meeting = await invoke("api_get_meeting", { meetingId: selected.id });
        if ((meeting?.transcripts?.length ?? 0) > 0) break;
      }
      await sleep(1000);
    }
    if (!selected?.id || !meeting || (meeting.transcripts?.length ?? 0) === 0) {
      throw new Error("No finalized meeting with transcripts was created by this recording");
    }
    state.meeting_id = selected.id;
    state.meeting_original_title = meeting.title;
    state.meeting_title_prefix = prefix;
    state.meeting_transcript_sha256 = sha256Text(transcriptText(meeting));
    saveState();
    const transcriptionStatus = await invoke("get_transcription_status", {});
    report = { selected, meeting, transcript_sha256: state.meeting_transcript_sha256,
      final_transcript: transcriptText(meeting),
      transcript_character_count: transcriptText(meeting).length,
      chunks_in_queue_at_finalization: transcriptionStatus?.chunks_in_queue ?? null,
      transcription_status_at_finalization: transcriptionStatus,
      finalization_completed: transcriptionStatus?.chunks_in_queue === 0
        && transcriptionStatus?.is_processing === false,
      time_audit: timeAudit(meeting.transcripts, request.expected_duration_seconds ?? null) };
  } else if (action === "meeting-snapshot") {
    const meeting = await invoke("api_get_meeting", { meetingId: requireMeetingId() });
    report = { meeting, transcript_sha256: sha256Text(transcriptText(meeting)),
      transcript_character_count: transcriptText(meeting).length,
      time_audit: timeAudit(meeting?.transcripts, request.expected_duration_seconds ?? null) };
  } else if (action === "manual-edit") {
    const meetingId = requireMeetingId();
    const title = String(request.title ?? `MOSS-FT-${state.run_id}`);
    const marker = String(request.transcript_marker ?? ` [manual-${state.run_id}]`);
    await navigate(`http://tauri.localhost/meeting-details?id=${encodeURIComponent(meetingId)}&source=recording`);
    report = await evaluate(`(async () => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const meetingId = ${JSON.stringify(meetingId)};
      const title = ${JSON.stringify(title)};
      const marker = ${JSON.stringify(marker)};
      const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
      const wait = async (fn, label, timeout = 15000) => {
        const started = performance.now();
        while (performance.now() - started < timeout) { const value = await fn(); if (value) return value; await sleep(50); }
        throw new Error('Timed out waiting for ' + label);
      };
      const setValue = (element, value) => {
        const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
        const setter = Object.getOwnPropertyDescriptor(prototype, 'value')?.set;
        if (!setter) throw new Error('Native value setter unavailable');
        setter.call(element, value);
        element.dispatchEvent(new Event('input', { bubbles: true }));
        element.dispatchEvent(new Event('change', { bubbles: true }));
      };
      const before = await invoke('api_get_meeting', { meetingId });
      await wait(() => document.querySelector('[data-transcript-id]'), 'transcript segment');
      const titleEdit = [...document.querySelectorAll('button')].filter((button) => button.getAttribute('aria-label') === '编辑会议标题');
      if (titleEdit.length !== 1) throw new Error('Expected one title edit control');
      titleEdit[0].click();
      const titleInput = await wait(() => [...document.querySelectorAll('textarea')].find((item) => item.className.includes('text-2xl')), 'title editor');
      setValue(titleInput, title); await sleep(100); titleInput.blur();
      await wait(async () => (await invoke('api_get_meeting', { meetingId }))?.title === title, 'title persistence');
      const segment = document.querySelector('[data-transcript-id]');
      const segmentId = segment.dataset.transcriptId;
      const edit = segment.querySelector('button[aria-label="编辑转写片段"]');
      if (!edit) throw new Error('Transcript edit control not found');
      edit.click();
      const textarea = await wait(() => segment.querySelector('textarea[aria-label="编辑转写片段文本"]'), 'transcript editor');
      const originalText = textarea.value;
      setValue(textarea, originalText + marker);
      const save = segment.querySelector('button[aria-label="保存校正"]');
      if (!save) throw new Error('Transcript save control not found');
      save.click();
      const after = await wait(async () => {
        const current = await invoke('api_get_meeting', { meetingId });
        return current?.transcripts?.find((item) => item.id === segmentId)?.text === originalText + marker ? current : null;
      }, 'transcript persistence');
      return { before, after, title, marker, segmentId, originalText,
        editedText: after.transcripts.find((item) => item.id === segmentId)?.text,
        verdict: { titleSaved: after.title === title,
          transcriptSavedExactly: after.transcripts.find((item) => item.id === segmentId)?.text === originalText + marker } };
    })()`);
    state.expected_title = report.title;
    state.edited_transcript_id = report.segmentId;
    state.expected_edited_transcript = report.editedText;
    state.expected_edited_transcript_sha256 = sha256Text(report.editedText);
    state.manual_edit_updated_at = report.after?.updated_at ?? report.after?.updatedAt ?? null;
    saveState();
  } else if (action === "persistence-verify") {
    const meetingId = requireMeetingId();
    const meeting = await invoke("api_get_meeting", { meetingId });
    const edited = meeting?.transcripts?.find((item) => item.id === state.edited_transcript_id) ?? null;
    const summaries = await invoke("api_list_summary_generation_history", { meetingId });
    report = { meeting, edited, summaries,
      title_exact: meeting?.title === state.expected_title,
      transcript_exact: edited?.text === state.expected_edited_transcript,
      transcript_sha256: sha256Text(edited?.text ?? ""),
      updated_at_exact: (meeting?.updated_at ?? meeting?.updatedAt ?? null) === state.manual_edit_updated_at,
      completed_summary_count: summaries.filter((item) => item.status === "completed").length };
  } else if (action === "whisper-enhance") {
    const meetingId = requireMeetingId();
    const timeoutMs = Math.min(Number(request.timeout_ms ?? PERFORMANCE_GATES.whisper_enhancement_completion),
      PERFORMANCE_GATES.whisper_enhancement_completion);
    await navigate(`http://tauri.localhost/meeting-details?id=${encodeURIComponent(meetingId)}&source=recording`);
    const nodeStartedMonotonicMs = performance.now();
    report = await evaluate(`(async () => {
      const invoke = window.__TAURI_INTERNALS__.invoke;
      const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
      const meetingId = ${JSON.stringify(meetingId)};
      const timeoutMs = ${JSON.stringify(timeoutMs)};
      const thresholdMs = ${JSON.stringify(PERFORMANCE_GATES.whisper_enhancement_completion)};
      const feedbackThresholdMs = ${JSON.stringify(PERFORMANCE_GATES.enhance_feedback)};
      const clock = 'webview_performance_now';
      const visible = (element) => { const r = element.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
      const before = await invoke('api_get_meeting', { meetingId });
      const beforeText = (before?.transcripts ?? []).map((item) => item.text ?? '').join('\n');
      const enhanceButtons = [...document.querySelectorAll('button')].filter((button) => {
        const label = button.getAttribute('aria-label') ?? '';
        const text = button.textContent?.trim() ?? '';
        return visible(button) && !button.disabled && (text === '增强' || label.includes('增强')
          || text === 'Enhance' || label.includes('Enhance'));
      });
      if (enhanceButtons.length !== 1) throw new Error('Expected exactly one enabled Enhance control, got ' + enhanceButtons.length);
      enhanceButtons[0].click();
      const dialogDeadline = performance.now() + 30000;
      let startButton = null;
      let comboValues = [];
      while (performance.now() <= dialogDeadline) {
        const standardTab = [...document.querySelectorAll('[role="tab"]')].find((tab) =>
          visible(tab) && (tab.textContent?.includes('常规重新转写') || tab.textContent?.includes('Standard retranscription')));
        if (standardTab && standardTab.getAttribute('data-state') !== 'active') standardTab.click();
        comboValues = [...document.querySelectorAll('button[role="combobox"]')].filter(visible)
          .map((button) => button.innerText.trim());
        startButton = [...document.querySelectorAll('button')].find((button) => visible(button) && !button.disabled
          && (button.innerText.trim() === '开始重新转写' || button.innerText.trim() === 'Start retranscription')) ?? null;
        if (startButton && comboValues.length >= 2) break;
        await sleep(50);
      }
      if (!startButton) throw new Error('Enabled standard retranscription action was not found');
      if (!comboValues.some((value) => value.includes('large-v3-turbo-q5_0'))) {
        throw new Error('Expected Whisper model is not selected: ' + JSON.stringify(comboValues));
      }
      const startedAt = new Date().toISOString();
      const startedMonotonicMs = performance.now();
      startButton.click();
      let feedbackMonotonicMs = null;
      let seenBusy = false;
      const observations = [];
      const feedbackDeadline = startedMonotonicMs + feedbackThresholdMs;
      while (performance.now() <= feedbackDeadline) {
        const busy = await invoke('is_retranscription_in_progress_command', {});
        const body = document.body?.innerText ?? '';
        seenBusy ||= busy === true;
        if (busy === true || body.includes('正在重新转写') || body.includes('Retranscribing')) {
          feedbackMonotonicMs = performance.now();
          break;
        }
        await sleep(20);
      }
      const feedback = {
        clock, started_monotonic_ms: startedMonotonicMs,
        completed_monotonic_ms: feedbackMonotonicMs,
        elapsed_ms: feedbackMonotonicMs === null ? null : feedbackMonotonicMs - startedMonotonicMs,
        threshold_ms: feedbackThresholdMs,
        within_threshold: feedbackMonotonicMs !== null
          && feedbackMonotonicMs - startedMonotonicMs <= feedbackThresholdMs,
      };
      let completedMonotonicMs = performance.now();
      let terminal = false;
      let cancelledAtThreshold = false;
      if (feedback.within_threshold) {
        while (performance.now() - startedMonotonicMs <= timeoutMs) {
          const busy = await invoke('is_retranscription_in_progress_command', {});
          seenBusy ||= busy === true;
          const sample = { at: new Date().toISOString(), monotonic_ms: performance.now(), busy };
          if (observations.length === 0 || observations.at(-1).busy !== sample.busy) observations.push(sample);
          if (seenBusy && busy === false) { terminal = true; break; }
          await sleep(100);
        }
        completedMonotonicMs = performance.now();
        if (!terminal && completedMonotonicMs - startedMonotonicMs >= thresholdMs) {
          cancelledAtThreshold = true;
          try { await invoke('cancel_retranscription_command', {}); } catch (_) { /* recorded below */ }
        }
      } else {
        const busy = await invoke('is_retranscription_in_progress_command', {});
        if (busy) {
          try { await invoke('cancel_retranscription_command', {}); } catch (_) { /* recorded below */ }
        }
      }
      const after = await invoke('api_get_meeting', { meetingId });
      const afterText = (after?.transcripts ?? []).map((item) => item.text ?? '').join('\n');
      const completion = {
        clock, started_monotonic_ms: startedMonotonicMs,
        completed_monotonic_ms: completedMonotonicMs,
        elapsed_ms: completedMonotonicMs - startedMonotonicMs,
        threshold_ms: thresholdMs,
        within_threshold: terminal && completedMonotonicMs - startedMonotonicMs <= thresholdMs,
        threshold_exceeded: cancelledAtThreshold,
      };
      return {
        startedAt, finishedAt: new Date().toISOString(), selected: comboValues,
        before, after, observations, seenBusy, cancelledAtThreshold,
        before_transcript_text: beforeText,
        after_transcript_text: afterText,
        performance: { enhance_feedback: feedback, whisper_enhancement_completion: completion },
        action_completed_monotonic_ms: performance.now(),
        verdict: { feedbackWithinThreshold: feedback.within_threshold,
          completedWithinThreshold: completion.within_threshold,
          transcriptPersisted: Array.isArray(after?.transcripts) && after.transcripts.length > 0 },
      };
    })()`, timeoutMs + 40000);
    const nodeCompletedMonotonicMs = performance.now();
    report.performance.whisper_enhancement_completion = performanceSample(nodeStartedMonotonicMs,
      nodeCompletedMonotonicMs, PERFORMANCE_GATES.whisper_enhancement_completion);
    report.performance.whisper_enhancement_completion.within_threshold &&=
      report.verdict.completedWithinThreshold === true;
    report.performance.whisper_enhancement_completion.threshold_exceeded =
      nodeCompletedMonotonicMs - nodeStartedMonotonicMs > PERFORMANCE_GATES.whisper_enhancement_completion;
    report.before_transcript_sha256 = sha256Text(report.before_transcript_text ?? "");
    report.after_transcript_sha256 = sha256Text(report.after_transcript_text ?? "");
    delete report.before_transcript_text;
    delete report.after_transcript_text;
    state.whisper_enhancement_runs = [...(state.whisper_enhancement_runs ?? []), {
      at: new Date().toISOString(), performance: report.performance,
      before_transcript_sha256: report.before_transcript_sha256,
      after_transcript_sha256: report.after_transcript_sha256,
    }];
    saveState();
  } else if (action === "inference-exclusion") {
    const meetingId = requireMeetingId();
    const operationSamples = [];
    const sampleOperations = async (label) => {
      const snapshot = await invoke("get_storage_operation_lock_status", {});
      const operations = snapshot?.activeOperations ?? snapshot?.active_operations ?? [];
      operationSamples.push({ label, monotonic_ms: performance.now(), operations,
        active_roles: operations.map((item) => item.operation)
          .map((item) => item === "moss_enhancement" ? "moss" : item === "summary_generation" ? "qwen" : null)
          .filter(Boolean) });
      return snapshot;
    };
    const attempts = [];
    const mossStarted = await invoke("api_moss_start_run", { request: { meetingId } });
    const mossRun = mossStarted.runs?.find((item) => ["preparing", "running"].includes(item.state)) ?? null;
    if (!mossRun?.runId) throw new Error("Mutual-exclusion probe could not start MOSS");
    await sampleOperations("moss-active-before-qwen-attempt");
    const modelConfig = await invoke("api_get_model_config", {});
    const qwenWhileMoss = await invokeRaw("api_process_transcript", {
      text: "mutual exclusion probe",
      model: modelConfig.provider,
      modelName: modelConfig.model,
      meetingId,
      chunkSize: 40000,
      overlap: 1000,
      customPrompt: "",
      templateId: null,
      historicalGenerationId: null,
      summaryLanguage: "zh-CN",
      authToken: null,
    });
    attempts.push({ requested: "qwen", while_active: "moss",
      outcome: qwenWhileMoss.ok ? "started" : "rejected",
      code: qwenWhileMoss.value?.code ?? null, raw: qwenWhileMoss });
    await sampleOperations("after-qwen-while-moss-attempt");
    if (qwenWhileMoss.ok) await invoke("api_cancel_summary", { meetingId });
    await invoke("api_moss_cancel_run", { request: { meetingId, runId: mossRun.runId } });
    await pollMoss(mossRun.runId, Number(request.timeout_ms ?? 120000));
    await sampleOperations("after-moss-cancelled");

    const transcript = await invoke("api_get_meeting", { meetingId });
    const transcriptPayload = (transcript?.transcripts ?? []).map((item) => item.text ?? "").join("\n");
    const summaryStarted = await invokeRaw("api_process_transcript", {
      text: transcriptPayload,
      model: modelConfig.provider,
      modelName: modelConfig.model,
      meetingId,
      chunkSize: 40000,
      overlap: 1000,
      customPrompt: "",
      templateId: null,
      historicalGenerationId: null,
      summaryLanguage: "zh-CN",
      authToken: null,
    });
    if (!summaryStarted.ok) throw new Error(`Mutual-exclusion probe could not start Qwen: ${summaryStarted.error}`);
    await sampleOperations("qwen-active-before-moss-attempt");
    const mossWhileQwen = await invokeRaw("api_moss_start_run", { request: { meetingId } });
    attempts.push({ requested: "moss", while_active: "qwen",
      outcome: mossWhileQwen.ok ? "started" : "rejected",
      code: mossWhileQwen.value?.code ?? null, raw: mossWhileQwen });
    await sampleOperations("after-moss-while-qwen-attempt");
    if (mossWhileQwen.ok) {
      const unexpectedRun = mossWhileQwen.value?.runs?.find((item) => ["preparing", "running"].includes(item.state));
      if (unexpectedRun?.runId) {
        await invoke("api_moss_cancel_run", { request: { meetingId, runId: unexpectedRun.runId } });
      }
    }
    await invoke("api_cancel_summary", { meetingId });
    const clearDeadline = performance.now() + 5000;
    while (performance.now() <= clearDeadline) {
      const snapshot = await sampleOperations("qwen-cancellation-poll");
      const operations = snapshot?.activeOperations ?? snapshot?.active_operations ?? [];
      if (!operations.some((item) => item.operation === "summary_generation")) break;
      await sleep(50);
    }
    const overlapDetected = operationSamples.some((item) => item.active_roles.includes("moss")
      && item.active_roles.includes("qwen"));
    let overlapMs = 0;
    for (let index = 0; index + 1 < operationSamples.length; index += 1) {
      if (operationSamples[index].active_roles.includes("moss")
        && operationSamples[index].active_roles.includes("qwen")) {
        overlapMs += Math.max(0, operationSamples[index + 1].monotonic_ms - operationSamples[index].monotonic_ms);
      }
    }
    report = { mossStarted, summaryStarted, attempts, operationSamples,
      action_completed_system_monotonic_ms: systemMonotonicMs(),
      moss_and_qwen_overlap_seconds: overlapMs / 1000,
      verdict: {
        qwenRejectedWhileMoss: qwenWhileMoss.ok === false && qwenWhileMoss.value?.code === "STORAGE_OPERATION_BUSY",
        mossRejectedWhileQwen: mossWhileQwen.ok === false && mossWhileQwen.value?.code === "MOSS_QWEN_BUSY",
        noOverlap: overlapDetected === false && overlapMs === 0,
      } };
  } else if (action === "moss-cancel") {
    const meetingId = requireMeetingId();
    const started = await startMossFromVisibleUi(meetingId);
    const enhanceFeedback = started.performance;
    const run = started.run;
    if (!run?.runId) throw new Error("MOSS start returned no run ID");
    if (enhanceFeedback.within_threshold) {
      await sleep(Math.max(250, Number(request.cancel_after_ms ?? 1500)));
    }
    let cancelled;
    if (enhanceFeedback.within_threshold) {
      cancelled = await cancelMossFromVisibleUi();
    } else {
      const cleanupStartedMonotonicMs = performance.now();
      await invoke("api_moss_cancel_run", { request: { meetingId, runId: run.runId } });
      const cleanupCompletedMonotonicMs = performance.now();
      const cleanupTiming = performanceSample(cleanupStartedMonotonicMs, cleanupCompletedMonotonicMs,
        PERFORMANCE_GATES.cancel_feedback);
      cleanupTiming.within_threshold = false;
      cleanupTiming.not_a_formal_ui_sample = true;
      cancelled = { ui: null, performance: cleanupTiming };
    }
    const cancelFeedback = cancelled.performance;
    const terminal = await pollMoss(run.runId, Number(request.timeout_ms ?? 120000));
    if (terminal.run?.state !== "cancelled") throw new Error(`Cancelled MOSS run ended as ${terminal.run?.state}`);
    state.cancelled_moss_run_id = run.runId;
    saveState();
    report = { started, cancelled, terminal,
      action_completed_system_monotonic_ms: systemMonotonicMs(),
      action_completed_monotonic_ms: performance.now(),
      performance: { enhance_feedback: enhanceFeedback, cancel_feedback: cancelFeedback }, verdict: {
      cancelled: terminal.run.state === "cancelled",
      noCandidate: terminal.run.candidateRevision == null && terminal.current.review == null,
      enhanceFeedbackWithinThreshold: enhanceFeedback.within_threshold,
      cancelFeedbackWithinThreshold: cancelFeedback.within_threshold,
      enhancementFeedbackWasVisibleUi: started.ui.uiProcessingVisible === true,
      cancellationFeedbackWasVisibleUi: cancelled.ui?.uiCancellationVisible === true,
      pageUsableAfterCancellation: cancelled.ui?.pageUsableAfterCancellation === true,
    } };
  } else if (action === "moss-complete") {
    const meetingId = requireMeetingId();
    const started = await startMossFromVisibleUi(meetingId);
    const enhanceFeedback = started.performance;
    const run = started.run;
    if (!run?.runId) throw new Error("MOSS start returned no run ID");
    if (!enhanceFeedback.within_threshold) {
      await invoke("api_moss_cancel_run", { request: { meetingId, runId: run.runId } });
      const terminal = await pollMoss(run.runId, Number(request.timeout_ms ?? 120000));
      report = { started, terminal, performance: { enhance_feedback: enhanceFeedback },
        action_completed_system_monotonic_ms: systemMonotonicMs(),
        action_completed_monotonic_ms: performance.now(), verdict: {
          completed: false, enhanceFeedbackWithinThreshold: false, cancelledAfterThresholdFailure: true,
          enhancementFeedbackWasVisibleUi: started.ui.uiProcessingVisible === true,
        } };
    } else {
    const terminal = await pollMoss(run.runId, Number(request.timeout_ms ?? 7200000));
    const percentages = terminal.samples.map((item) => item.percentage).filter(Number.isFinite);
    const monotonic = percentages.every((item, index) => index === 0 || item >= percentages[index - 1]);
    const candidate = terminal.current.review?.candidate ?? null;
    if (terminal.run?.state !== "completed" || !candidate || candidate.runId !== run.runId) {
      throw new Error("MOSS run did not complete with its own candidate");
    }
    const decodeContract = nativeDecodeContract(terminal.run);
    state.moss_run_id = run.runId;
    state.moss_candidate_revision = candidate.revision;
    state.moss_candidate_sha256 = candidate.sha256 ?? sha256Text(JSON.stringify(candidate.segments ?? []));
    saveState();
    report = { started, terminal, native_decode_contract: decodeContract,
      performance: { enhance_feedback: enhanceFeedback },
      action_completed_system_monotonic_ms: systemMonotonicMs(),
      action_completed_monotonic_ms: performance.now(),
      candidate_time_audit: timeAudit(candidate.segments, request.expected_duration_seconds ?? null),
      verdict: { completed: true, progressMonotonic: monotonic, progressSamplesPresent: percentages.length >= 2,
        candidateStored: true, currentTranscriptNotAutoReplaced: candidate.isActive === false,
        enhanceFeedbackWithinThreshold: true,
        enhancementFeedbackWasVisibleUi: started.ui.uiProcessingVisible === true } };
    }
  } else if (action === "moss-review-q00") {
    const meetingId = requireMeetingId();
    const requestedBindings = request.speaker_bindings;
    const overrideSegmentId = request.override_segment_id;
    const overridePersonId = request.override_person_id;
    if (!Array.isArray(requestedBindings) || requestedBindings.length === 0
      || !requestedBindings.every((item) => item && typeof item.speaker_label === "string"
        && item.speaker_label && typeof item.person_id === "string" && item.person_id)) {
      throw new Error("moss-review-q00 requires non-empty exact speaker_bindings");
    }
    if (typeof overrideSegmentId !== "string" || !overrideSegmentId
      || typeof overridePersonId !== "string" || !overridePersonId) {
      throw new Error("moss-review-q00 requires one exact manual segment override");
    }
    const labels = requestedBindings.map((item) => item.speaker_label);
    if (new Set(labels).size !== labels.length) throw new Error("Q00 speaker bindings contain a duplicate label");
    const before = await workspace();
    let review = before.review;
    if (!review?.candidate || review.candidate.isActive !== false) {
      throw new Error("Q00 requires one separate inactive MOSS candidate");
    }
    const candidateLabels = [...new Set(review.candidate.segments
      .map((item) => item.speakerLabel).filter((item) => typeof item === "string" && item))].sort();
    if (JSON.stringify([...labels].sort()) !== JSON.stringify(candidateLabels)) {
      throw new Error("Q00 speaker bindings do not cover the exact candidate label set");
    }
    const participantIds = new Set((review.participants ?? []).map((item) => item.personId));
    if (requestedBindings.some((item) => !participantIds.has(item.person_id))
      || !participantIds.has(overridePersonId)) {
      throw new Error("Q00 speaker assignment references a person outside the frozen meeting context");
    }
    for (const binding of requestedBindings) {
      const updated = await invoke("api_moss_save_speaker_binding", { request: {
        meetingId,
        runId: review.candidate.runId,
        speakerLabel: binding.speaker_label,
        personId: binding.person_id,
        expectedCandidateRevision: review.candidate.revision,
      } });
      review = updated.review;
    }
    if (!review.candidate.segments.some((item) => item.segmentId === overrideSegmentId)) {
      throw new Error("Q00 override segment is not part of the bound candidate");
    }
    const overridden = await invoke("api_moss_save_segment_override", { request: {
      meetingId,
      runId: review.candidate.runId,
      segmentId: overrideSegmentId,
      personId: overridePersonId,
      expectedCandidateRevision: review.candidate.revision,
    } });
    review = overridden.review;
    const bindingByLabel = new Map(requestedBindings.map((item) => [item.speaker_label, item.person_id]));
    const wrong = review.candidate.segments.filter((item) => {
      const expected = item.segmentId === overrideSegmentId
        ? overridePersonId : bindingByLabel.get(item.speakerLabel);
      return item.resolvedPersonId !== expected;
    });
    if (wrong.length !== 0) throw new Error("Q00 persisted speaker resolution differs from the approved mapping");
    state.review_expected = {
      revision: review.candidate.revision,
      speaker_bindings: requestedBindings,
      override_segment_id: overrideSegmentId,
      override_person_id: overridePersonId,
    };
    saveState();
    report = {
      before,
      finalWorkspace: overridden,
      correctionCount: review.corrections?.length ?? 0,
      verdict: {
        candidateSeparate: true,
        exactSpeakerLabelCoverage: true,
        everySpeakerBindingPersisted: true,
        oneManualSegmentOverridePersisted: true,
      },
    };
  } else if (action === "moss-review-strict") {
    const meetingId = requireMeetingId();
    const positiveTerms = request.positive_terms;
    const negativeTerms = request.negative_terms;
    if (!Array.isArray(positiveTerms) || positiveTerms.length === 0
      || !positiveTerms.every((item) => typeof item === "string" && item.trim())) {
      throw new Error("Strict positive_terms are required");
    }
    if (!Array.isArray(negativeTerms) || negativeTerms.length === 0
      || !negativeTerms.every((item) => typeof item === "string" && item.trim())) {
      throw new Error("Strict negative_terms are required");
    }
    const before = await workspace();
    let review = before.review;
    if (!review?.candidate || review.candidate.isActive !== false) throw new Error("A separate inactive candidate is required");
    if (!Array.isArray(review.corrections) || review.corrections.length === 0) {
      throw new Error("MOSS corrections are absent; strict correction tests cannot pass");
    }
    if (!Array.isArray(review.participants) || review.participants.length < 2) {
      throw new Error("At least two real participants are required for strict speaker tests");
    }
    const speakerSegment = review.candidate.segments?.find((item) => typeof item.speakerLabel === "string" && item.speakerLabel);
    if (!speakerSegment) throw new Error("No candidate segment has a speaker label");
    const candidateText = review.candidate.segments.map((item) => item.text ?? "").join("\n");
    const correctionJson = JSON.stringify(review.corrections);
    const positive = positiveTerms.map((term) => ({ term, candidate_count: candidateText.split(term).length - 1,
      correction_trace_count: correctionJson.split(term).length - 1 }));
    if (positive.some((item) => item.candidate_count < 1 || item.correction_trace_count < 1)) {
      throw new Error("A strict positive term is missing from candidate or correction trace");
    }
    const negative = negativeTerms.map((term) => ({ term, candidate_count: candidateText.split(term).length - 1 }));
    if (negative.some((item) => item.candidate_count !== 0)) throw new Error("A frozen negative term was inserted");

    const editable = review.candidate.segments.find((item) => String(item.text ?? "").trim());
    if (!editable) throw new Error("Candidate has no editable segment");
    const originalRevision = review.candidate.revision;
    const marker = ` [candidate-manual-${state.run_id}]`;
    const editedText = `${editable.text}${marker}`;
    let current = await invoke("api_moss_update_candidate_segment", { request: {
      meetingId, runId: review.candidate.runId, segmentId: editable.segmentId,
      text: editedText, expectedCandidateRevision: originalRevision,
    } });
    const conflict = await invokeRaw("api_moss_update_candidate_segment", { request: {
      meetingId, runId: review.candidate.runId, segmentId: editable.segmentId,
      text: `${editedText}-stale`, expectedCandidateRevision: originalRevision,
    } });
    if (conflict.ok || conflict.value?.code !== "MOSS_CANDIDATE_CONFLICT") {
      throw new Error("Stale candidate revision was not rejected with MOSS_CANDIDATE_CONFLICT");
    }
    const personA = review.participants[0];
    const personB = review.participants[1];
    current = await invoke("api_moss_save_speaker_binding", { request: {
      meetingId, runId: review.candidate.runId, speakerLabel: speakerSegment.speakerLabel,
      personId: personA.personId, expectedCandidateRevision: current.review.candidate.revision,
    } });
    current = await invoke("api_moss_save_segment_override", { request: {
      meetingId, runId: review.candidate.runId, segmentId: speakerSegment.segmentId,
      personId: personB.personId, expectedCandidateRevision: current.review.candidate.revision,
    } });
    const applied = current.review.corrections.find((item) => item.state === "applied");
    if (!applied?.correctionId) throw new Error("No applied correction exists for strict round-trip testing");
    const reverted = await invoke("api_moss_set_correction_state", { request: {
      meetingId, runId: review.candidate.runId, correctionId: applied.correctionId, applied: false,
      expectedCandidateRevision: current.review.candidate.revision,
    } });
    const reapplied = await invoke("api_moss_set_correction_state", { request: {
      meetingId, runId: review.candidate.runId, correctionId: applied.correctionId, applied: true,
      expectedCandidateRevision: reverted.review.candidate.revision,
    } });
    review = reapplied.review;
    const finalEdited = review.candidate.segments.find((item) => item.segmentId === editable.segmentId);
    const finalOverride = review.candidate.segments.find((item) => item.segmentId === speakerSegment.segmentId);
    const binding = review.bindings.find((item) => item.speakerLabel === speakerSegment.speakerLabel);
    const finalCorrection = review.corrections.find((item) => item.correctionId === applied.correctionId);
    if (finalEdited?.text !== editedText || finalEdited?.textSourceLayer !== "human_edit"
      || binding?.personId !== personA.personId
      || finalOverride?.resolvedPersonId !== personB.personId
      || finalOverride?.speakerResolution !== "segment_override"
      || finalCorrection?.state !== "applied") {
      throw new Error("Strict review persistence checks failed immediately after mutation");
    }
    state.review_expected = {
      revision: review.candidate.revision, edited_segment_id: editable.segmentId,
      edited_text: editedText, speaker_label: speakerSegment.speakerLabel,
      binding_person_id: personA.personId, override_segment_id: speakerSegment.segmentId,
      override_person_id: personB.personId, correction_id: applied.correctionId,
    };
    saveState();
    report = { before, positive, negative, conflict, finalWorkspace: reapplied,
      strict: { corrections_count: review.corrections.length, participants_count: review.participants.length,
        speaker_label_count: new Set(review.candidate.segments.map((item) => item.speakerLabel).filter(Boolean)).size,
        binding, override: finalOverride, correction: finalCorrection, edited: finalEdited },
      verdict: { candidateSeparate: true, positiveTermsTraceable: true, negativeInsertionsZero: true,
        manualEditPersisted: true, staleRevisionRejected: true, speakerBindingApplied: true,
        segmentOverrideWins: true, correctionRoundTrip: true } };
  } else if (action === "moss-workspace-verify") {
    const current = await workspace();
    const expected = state.review_expected;
    if (!expected) throw new Error("review_expected is missing from state");
    const review = current.review;
    const edited = review?.candidate?.segments?.find((item) => item.segmentId === expected.edited_segment_id);
    const overridden = review?.candidate?.segments?.find((item) => item.segmentId === expected.override_segment_id);
    const binding = review?.bindings?.find((item) => item.speakerLabel === expected.speaker_label);
    const correction = review?.corrections?.find((item) => item.correctionId === expected.correction_id);
    report = { current, verdict: {
      revisionExact: review?.candidate?.revision === expected.revision,
      editedTextExact: edited?.text === expected.edited_text && edited?.textSourceLayer === "human_edit",
      bindingExact: binding?.personId === expected.binding_person_id,
      overrideExact: overridden?.resolvedPersonId === expected.override_person_id
        && overridden?.speakerResolution === "segment_override",
      correctionApplied: correction?.state === "applied",
    } };
  } else if (action === "moss-activate" || action === "moss-reactivate") {
    const before = await workspace();
    const review = before.review;
    if (!review?.candidate) throw new Error("No candidate is available for activation");
    const activated = await invoke("api_moss_activate_candidate", { request: {
      meetingId: requireMeetingId(), runId: review.candidate.runId,
      expectedCandidateRevision: review.candidate.revision,
      expectedCurrentTranscriptSha256: review.current.sha256,
    } });
    const activation = activated.review?.activation;
    if (activated.review?.candidate?.isActive !== true
      || activation?.activeRunId !== review.candidate.runId
      || !activation?.activeActivationId) throw new Error("Candidate activation was not exact");
    state.activation_id = activation.activeActivationId;
    state.activation_transcript_sha256 = activation.currentTranscriptSha256 ?? activated.review.current?.sha256;
    saveState();
    report = { before, activated, verdict: { activated: true, activeRunMatches: true, activationIdPresent: true } };
  } else if (action === "moss-rollback") {
    const before = await workspace();
    const activationId = before.review?.activation?.activeActivationId;
    if (!activationId) throw new Error("No active MOSS activation exists");
    const rolledBack = await invoke("api_moss_rollback_activation", { request: {
      meetingId: requireMeetingId(), activationId,
      expectedCurrentTranscriptSha256: before.review.activation.currentTranscriptSha256,
    } });
    if (rolledBack.review?.activation?.activeRunId !== null
      || rolledBack.review?.candidate?.isActive !== false
      || rolledBack.review?.activation?.canActivate !== true) throw new Error("MOSS rollback state is invalid");
    report = { before, rolledBack, verdict: { noActiveRun: true, candidateInactive: true, canReactivate: true } };
  } else if (action === "summary-generate") {
    report = await generateSummary(requireMeetingId(), Number(request.timeout_ms ?? 600000));
    state.summary_runs = [...(state.summary_runs ?? []), {
      at: report.finishedAt, generation_id: report.newest?.generationId ?? report.newest?.generation_id ?? null,
      source_binding: report.sourceBinding,
    }];
    saveState();
  } else if (action === "recording-preferences-set") {
    const root = request.recording_root;
    if (typeof root !== "string" || !root) throw new Error("recording_root is required");
    const before = await invoke("get_recording_preferences", {});
    await invoke("set_recording_preferences", { preferences: {
      ...before, save_folder: root, auto_save: true, file_format: "wav",
    } });
    const after = await invoke("get_recording_preferences", {});
    report = { before, after, applied: path.win32.normalize(after?.save_folder ?? "").toLowerCase()
      === path.win32.normalize(root).toLowerCase() };
  } else if (action === "recording-preferences-invalid") {
    const root = request.recording_root;
    if (typeof root !== "string" || !root) throw new Error("recording_root is required");
    const before = await invoke("get_recording_preferences", {});
    const attempted = await invokeRaw("set_recording_preferences", { preferences: {
      ...before, save_folder: root, auto_save: true, file_format: "wav",
    } });
    const after = await invoke("get_recording_preferences", {});
    report = { before, attempted, after, verdict: {
      explicitlyRejected: attempted.ok === false && Boolean(attempted.error),
      preferenceUnchanged: before?.save_folder === after?.save_folder,
    } };
  } else if (action === "fault-snapshot") {
    const meetingId = request.meeting_id ?? state.meeting_id ?? null;
    const recording = await invoke("get_recording_state", {});
    const storage = await invoke("get_storage_status", {});
    const moss = await invoke("api_moss_get_system_status", {});
    const meeting = meetingId ? await invoke("api_get_meeting", { meetingId }) : null;
    const summaries = meetingId ? await invoke("api_list_summary_generation_history", { meetingId }) : [];
    report = { recording, storage, moss, meeting, summaries,
      transcript_sha256: meeting ? sha256Text(transcriptText(meeting)) : null,
      summary_sha256: meeting ? sha256Text(JSON.stringify(meeting.summary ?? null)) : null };
  }

  report = {
    schema_version: 1,
    stage: "MOSS_FUNCTIONAL_FT_CDP",
    action,
    run_id: state.run_id,
    source_commit: state.source_commit,
    candidate_sha256: state.candidate_sha256,
    captured_at: new Date().toISOString(),
    cdp_target_id: cdpTargetId,
    ...report,
  };
  fs.mkdirSync(path.dirname(outputPath), { recursive: true });
  fs.writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`, { encoding: "utf8", flag: "wx" });
  console.log(JSON.stringify({ action, status: "EXECUTED", output_bytes: fs.statSync(outputPath).size }));
} finally {
  socket.close();
}
