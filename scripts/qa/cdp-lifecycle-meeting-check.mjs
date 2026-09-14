import crypto from "node:crypto";
import fs from "node:fs";

const bindingPath = process.argv[2];
const outputPath = process.argv[3];
const screenshotPath = process.argv[4];
if (!bindingPath || !outputPath) {
  throw new Error("Usage: node cdp-lifecycle-meeting-check.mjs <fixture-binding.json> <output.json> [screenshot.png]");
}
const binding = JSON.parse(fs.readFileSync(bindingPath, "utf8"));
if (!binding.meeting_id || !binding.meeting_title || !binding.transcript_fingerprint_sha256 || !binding.anchor_transcript_id) {
  throw new Error("Fixture binding is missing the expected meeting identity or transcript fingerprint");
}

const cdpPort = process.env.CDP_PORT;
const cdpTargetId = process.env.CDP_TARGET_ID;
if (!/^\d+$/.test(cdpPort ?? "") || !cdpTargetId) {
  throw new Error("CDP_PORT and the process-bound CDP_TARGET_ID are required");
}

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
function transcriptFingerprint(transcripts) {
  const lines = transcripts
    .map((transcript) => `${String(transcript.id)}|${String(transcript.text ?? transcript.transcript ?? "")}`)
    .sort();
  return crypto.createHash("sha256").update(lines.join("\n"), "utf8").digest("hex").toUpperCase();
}

const targetResponse = await fetch(`http://127.0.0.1:${cdpPort}/json/list`, {
  signal: AbortSignal.timeout(5000),
});
if (!targetResponse.ok) throw new Error(`CDP target list returned HTTP ${targetResponse.status}`);
const targets = await targetResponse.json();
const page = targets.find((target) => target.id === cdpTargetId);
if (!page || page.type !== "page") throw new Error("The exact process-bound WebView2 target was not found");
if (!(page.url.startsWith("http://tauri.localhost") || page.url.startsWith("http://localhost:"))) {
  throw new Error(`The process-bound target has an unexpected URL: ${page.url}`);
}

const socket = new WebSocket(page.webSocketDebuggerUrl);
let nextId = 1;
const pending = new Map();
function rejectPending(error) {
  for (const { reject, timer } of pending.values()) {
    clearTimeout(timer);
    reject(error);
  }
  pending.clear();
}
socket.addEventListener("message", (event) => {
  const message = JSON.parse(event.data);
  if (!message.id || !pending.has(message.id)) return;
  const { resolve, reject, timer } = pending.get(message.id);
  clearTimeout(timer);
  pending.delete(message.id);
  if (message.error) reject(new Error(JSON.stringify(message.error)));
  else resolve(message.result);
});
socket.addEventListener("close", () => rejectPending(new Error("CDP socket closed before the request completed")));
await new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error("Timed out opening the CDP socket")), 5000);
  socket.addEventListener("open", () => { clearTimeout(timer); resolve(); }, { once: true });
  socket.addEventListener("error", (error) => { clearTimeout(timer); reject(error); }, { once: true });
});

function call(method, params = {}, timeoutMilliseconds = 5000) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`CDP ${method} timed out after ${timeoutMilliseconds}ms`));
    }, timeoutMilliseconds);
    pending.set(id, { resolve, reject, timer });
    socket.send(JSON.stringify({ id, method, params }));
  });
}
async function evaluate(expression) {
  const evaluated = await call("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
    userGesture: true,
  });
  if (evaluated.exceptionDetails) {
    throw new Error(evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text);
  }
  return evaluated.result.value;
}

try {
  await call("Runtime.enable");
  await call("Page.enable");

  const listSamples = [];
  const listDeadline = Date.now() + 15000;
  let meetings = null;
  let selected = null;
  while (Date.now() < listDeadline) {
    try {
      meetings = await evaluate(`(() => {
        if (!window.__TAURI_INTERNALS__?.invoke) throw new Error('Tauri invoke is not ready');
        return window.__TAURI_INTERNALS__.invoke('api_get_meetings');
      })()`);
      selected = Array.isArray(meetings) ? meetings.find((meeting) => meeting?.id === binding.meeting_id) : null;
      listSamples.push({ capturedAt: new Date().toISOString(), meetingCount: Array.isArray(meetings) ? meetings.length : null, selected: Boolean(selected) });
      if (selected?.title === binding.meeting_title) break;
    } catch (error) {
      listSamples.push({ capturedAt: new Date().toISOString(), error: String(error?.message ?? error) });
    }
    await delay(250);
  }
  if (!Array.isArray(meetings) || meetings.length === 0) throw new Error("The preserved database returned no meetings");
  if (!selected) throw new Error("The exact protected fixture meeting was not returned by api_get_meetings");
  if (selected.title !== binding.meeting_title) throw new Error("The protected fixture title changed");

  const destination = `http://tauri.localhost/meeting-details?id=${encodeURIComponent(binding.meeting_id)}&source=recording`;
  await call("Page.navigate", { url: destination }, 10000);
  const detailSamples = [];
  const detailDeadline = Date.now() + 20000;
  let details = null;
  while (Date.now() < detailDeadline) {
    try {
      details = await evaluate(`(async () => {
        if (!window.__TAURI_INTERNALS__?.invoke) throw new Error('Tauri invoke is not ready');
        const selectedId = ${JSON.stringify(binding.meeting_id)};
        const anchorTranscriptId = ${JSON.stringify(binding.anchor_transcript_id)};
        const meeting = await window.__TAURI_INTERNALS__.invoke('api_get_meeting', { meetingId: selectedId });
        const anchor = [...document.querySelectorAll('[data-transcript-id]')]
          .find((element) => element.dataset.transcriptId === anchorTranscriptId) ?? null;
        const anchorRect = anchor?.getBoundingClientRect() ?? null;
        return {
          capturedAt: new Date().toISOString(), href: location.href,
          routeMeetingId: new URL(location.href).searchParams.get('id'),
          heading: document.querySelector('h1')?.textContent?.trim() ?? null,
          anchorTranscriptPresent: anchor !== null,
          anchorTranscriptVisible: anchorRect !== null && anchorRect.width > 0 && anchorRect.height > 0,
          meeting,
        };
      })()`);
      detailSamples.push({
        capturedAt: details.capturedAt, routeMeetingId: details.routeMeetingId, heading: details.heading,
        anchorTranscriptPresent: details.anchorTranscriptPresent, anchorTranscriptVisible: details.anchorTranscriptVisible,
        apiMeetingId: details.meeting?.id ?? null,
      });
      if (details.routeMeetingId === binding.meeting_id && details.heading === binding.meeting_title &&
          details.anchorTranscriptPresent && details.anchorTranscriptVisible && details.meeting?.id === binding.meeting_id) break;
    } catch (error) {
      detailSamples.push({ capturedAt: new Date().toISOString(), error: String(error?.message ?? error) });
    }
    await delay(250);
  }
  if (!details) throw new Error("Meeting details polling produced no successful sample");

  if (screenshotPath) {
    const screenshot = await call("Page.captureScreenshot", { format: "png" }, 10000);
    fs.writeFileSync(screenshotPath, Buffer.from(screenshot.data, "base64"));
  }

  const report = {
    schemaVersion: 1, checkedAt: new Date().toISOString(), cdpTargetId, cdpTargetUrl: page.url,
    meetingCount: meetings.length, selectedMeeting: { id: binding.meeting_id, title: binding.meeting_title },
    expectedTranscriptCount: binding.transcript_count,
    expectedTranscriptFingerprintSha256: binding.transcript_fingerprint_sha256,
    actualTranscriptCount: details.meeting?.transcripts?.length ?? null,
    actualTranscriptFingerprintSha256: transcriptFingerprint(details.meeting?.transcripts ?? []),
    details, listSamples, detailSamples,
    verdict: {
      meetingListNonEmpty: meetings.length > 0,
      exactMeetingReturnedByList: selected.id === binding.meeting_id && selected.title === binding.meeting_title,
      selectedMeetingReturnedByApi: details.meeting?.id === binding.meeting_id && details.meeting?.title === binding.meeting_title,
      exactDetailsRouteOpened: details.routeMeetingId === binding.meeting_id,
      selectedTitleVisibleInDetailsHeading: details.heading === binding.meeting_title,
      boundTranscriptRenderedInDetails: details.anchorTranscriptPresent && details.anchorTranscriptVisible,
      transcriptCountUnchanged: (details.meeting?.transcripts?.length ?? -1) === binding.transcript_count,
      transcriptContentUnchanged: transcriptFingerprint(details.meeting?.transcripts ?? []) === binding.transcript_fingerprint_sha256,
    },
  };
  report.status = Object.values(report.verdict).every(Boolean) ? "PASS" : "FAIL";
  fs.writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
  console.log(JSON.stringify({ status: report.status, meetingCount: report.meetingCount, verdict: report.verdict, outputPath }, null, 2));
  if (report.status !== "PASS") process.exitCode = 1;
} finally {
  socket.close();
}
