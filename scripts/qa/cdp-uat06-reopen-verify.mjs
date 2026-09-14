import fs from "node:fs";
import crypto from "node:crypto";

const meetingId = process.argv[2];
const mode = process.argv[3] ?? "current";
const outputPath = process.argv[4];
const screenshotPath = process.argv[5];
if (!meetingId || !outputPath || !["route", "current"].includes(mode)) {
  throw new Error(
    "Usage: node cdp-uat06-reopen-verify.mjs <meeting-id> <route|current> <output.json> [screenshot.png]",
  );
}

const expectedCorrection =
  "本周仍计划签约 8 个 YouTube 新博主；后续量化目标待下周根据实际数据表现确定。";
const transcriptId = "transcript-b83f75c4-4cdb-43df-9123-38235dae426a";
const cdpPort = process.env.CDP_PORT ?? "9233";
const pidBefore = Number(process.env.UAT_PID_BEFORE ?? 0);
const pidAfter = Number(process.env.UAT_PID_AFTER ?? 0);
const executablePath = process.env.UAT_EXE_PATH ?? "";
const expectedReleaseSha256 = process.env.UAT_EXE_SHA256 ?? "";
const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`).then((response) =>
  response.json(),
);
const page = targets.find(
  (target) => target.type === "page" && target.url.startsWith("http://tauri.localhost"),
);
if (!page) throw new Error("Meetily WebView2 debug target was not found");

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
async function waitForPage(expectedPart, timeoutMs = 30000) {
  const started = Date.now();
  let state = null;
  while (Date.now() - started < timeoutMs) {
    const evaluated = await call("Runtime.evaluate", {
      expression: `({ href: location.href, text: document.body?.innerText ?? '' })`,
      returnByValue: true,
    });
    state = evaluated.result.value;
    if (state.href.includes(expectedPart) && state.text.length > 100) return state;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Timed out waiting for page: ${expectedPart}; last=${state?.href}`);
}

await call("Runtime.enable");
await call("Page.enable");
await call("Emulation.setDeviceMetricsOverride", {
  width: 1600,
  height: 1000,
  deviceScaleFactor: 1,
  mobile: false,
});
if (mode === "route") {
  await call("Page.navigate", { url: "http://tauri.localhost/" });
  await new Promise((resolve) => setTimeout(resolve, 750));
}
const destination = `http://tauri.localhost/meeting-details?id=${encodeURIComponent(meetingId)}&source=recording`;
await call("Page.navigate", { url: destination });
await waitForPage(meetingId);

const expression = `(async () => {
  const meetingId = ${JSON.stringify(meetingId)};
  const expectedCorrection = ${JSON.stringify(expectedCorrection)};
  const transcriptId = ${JSON.stringify(transcriptId)};
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const [meeting, summary, history, manualRevisions, recording, retranscription] =
    await Promise.all([
      invoke('api_get_meeting', { meetingId }),
      invoke('api_get_summary', { meetingId }),
      invoke('api_list_summary_generation_history', { meetingId }),
      invoke('api_list_manual_summary_revisions', { meetingId }),
      invoke('get_recording_state'),
      invoke('is_retranscription_in_progress_command'),
    ]);
  const transcript = meeting.transcripts.find((segment) => segment.id === transcriptId) ?? null;
  const factValidation = summary?.data?.factValidation ?? null;
  const templateSnapshot = summary?.data?.template_snapshot ?? null;
  const latestRevision = manualRevisions[0] ?? null;
  const backendContainsCorrection = summary?.data?.markdown?.includes(expectedCorrection)
    || JSON.stringify(summary?.data?.summary_json ?? []).includes(expectedCorrection);
  const visibleText = document.body?.innerText ?? '';
  const visibleAlerts = [...document.querySelectorAll('[role="alert"]')]
    .filter((element) => {
      const rect = element.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    })
    .map((element) => element.innerText.trim());
  const warningCodes = factValidation?.warnings?.map((warning) => warning.code) ?? [];
  return {
    capturedAt: new Date().toISOString(),
    mode: ${JSON.stringify(mode)},
    href: location.href,
    meetingId,
    meeting,
    summary,
    history,
    manualRevisions,
    recording,
    retranscription,
    factValidation,
    templateSnapshot,
    latestRevision,
    transcript,
    visibleAlerts,
    visibleTextTail: visibleText.slice(-5000),
    verdict: {
      correctMeeting: location.href.includes(meetingId),
      correctionVisible: visibleText.includes(expectedCorrection),
      correctionPersistedInBackend: backendContainsCorrection,
      cancelledTranscriptEditAbsent: transcript?.text.includes('贴子') === true
        && transcript?.text.includes('帖子') === false,
      exactlyOneManualRevision: manualRevisions.length === 1,
      latestRevisionCurrent: latestRevision?.isCurrent === true,
      sourceGenerationPreserved: latestRevision?.sourceGenerationId === history[0]?.generationId,
      generationHistoryStillTwo: history.length === 2,
      factReviewPreserved: factValidation?.status === 'needs_review'
        && factValidation?.warningCount === 3
        && [
          'unsupported_transcript_term',
          'unsupported_organization',
          'unsupported_status_claim',
        ].every((code) => warningCodes.includes(code)),
      templateSnapshotPreserved: templateSnapshot?.generationId === history[0]?.generationId
        && templateSnapshot?.resolvedTemplate?.id === 'license_station_weekly'
        && templateSnapshot?.resolvedTemplate?.version === 4,
      factBannerVisible: visibleAlerts.some((text) => text.includes('需要核对事实')),
      recordingIdle: recording?.is_recording === false && recording?.is_active === false,
      retranscriptionIdle: retranscription === false,
    },
  };
})()`;
const evaluated = await call("Runtime.evaluate", {
  expression,
  returnByValue: true,
  awaitPromise: true,
});
if (evaluated.exceptionDetails) {
  throw new Error(evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text);
}
const result = evaluated.result.value;
if (pidBefore > 0 || pidAfter > 0 || executablePath) {
  const processExists = (pid) => {
    if (!(pid > 0)) return null;
    try {
      process.kill(pid, 0);
      return true;
    } catch (error) {
      if (error?.code === "ESRCH") return false;
      throw error;
    }
  };
  const releaseSha256 = executablePath
    ? crypto.createHash("sha256").update(fs.readFileSync(executablePath)).digest("hex").toUpperCase()
    : null;
  result.restartMetadata = {
    pidBefore: pidBefore || null,
    pidAfter: pidAfter || null,
    pidChanged: pidBefore > 0 && pidAfter > 0 && pidBefore !== pidAfter,
    oldProcessGone: pidBefore > 0 ? !processExists(pidBefore) : null,
    newProcessAlive: processExists(pidAfter),
    executablePath: executablePath || null,
    releaseSha256,
    expectedReleaseSha256: expectedReleaseSha256 || null,
    releaseHashMatchesExpected: expectedReleaseSha256
      ? releaseSha256 === expectedReleaseSha256.toUpperCase()
      : null,
    productionOrigin: result.href?.startsWith("http://tauri.localhost/") === true,
  };
}
const output = `${JSON.stringify(result, null, 2)}\n`;
fs.writeFileSync(outputPath, output, "utf8");
if (screenshotPath) {
  const screenshot = await call("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(screenshotPath, Buffer.from(screenshot.data, "base64"));
}
console.log(output.trimEnd());
socket.close();
