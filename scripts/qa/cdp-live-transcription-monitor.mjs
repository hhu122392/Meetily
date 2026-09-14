import fs from "node:fs";
import path from "node:path";

const outputPath = process.argv[2];
const durationMs = Number(process.argv[3] ?? 90000);
const pollMs = Number(process.argv[4] ?? 200);
if (!outputPath || !Number.isFinite(durationMs) || durationMs <= 0) {
  throw new Error(
    "Usage: node cdp-live-transcription-monitor.mjs <output.json> [duration-ms] [poll-ms]",
  );
}

const cdpPort = process.env.CDP_PORT ?? "9233";
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

await call("Runtime.enable");
const expression = `(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const durationMs = ${JSON.stringify(durationMs)};
  const pollMs = ${JSON.stringify(pollMs)};
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const startedAt = new Date().toISOString();
  const started = performance.now();
  const changes = [];
  const stateSamples = [];
  let lastFingerprint = null;
  let lastStateSampleAt = -1000;
  while (performance.now() - started < durationMs) {
    const elapsedMs = performance.now() - started;
    const [history, state, folder, meetingName] = await Promise.all([
      invoke('get_transcript_history', {}).catch((error) => ({ __error: String(error) })),
      invoke('get_recording_state', {}).catch((error) => ({ __error: String(error) })),
      invoke('get_meeting_folder_path', {}).catch(() => null),
      invoke('get_current_meeting_name', {}).catch(() => null),
    ]);
    if (Array.isArray(history)) {
      const fingerprint = JSON.stringify(history);
      if (fingerprint !== lastFingerprint) {
        const ordered = [...history].sort((left, right) =>
          (left.sequence_id ?? 0) - (right.sequence_id ?? 0));
        changes.push({
          at: new Date().toISOString(),
          elapsedMs,
          folder,
          meetingName,
          segmentCount: ordered.length,
          segments: ordered,
          text: ordered.map((segment) => segment.text ?? '').join(''),
        });
        lastFingerprint = fingerprint;
      }
    }
    if (elapsedMs - lastStateSampleAt >= 1000) {
      stateSamples.push({ at: new Date().toISOString(), elapsedMs, state, folder, meetingName });
      lastStateSampleAt = elapsedMs;
    }
    await sleep(pollMs);
  }
  const [finalHistory, finalState, finalFolder, finalMeetingName] = await Promise.all([
    invoke('get_transcript_history', {}).catch((error) => ({ __error: String(error) })),
    invoke('get_recording_state', {}).catch((error) => ({ __error: String(error) })),
    invoke('get_meeting_folder_path', {}).catch(() => null),
    invoke('get_current_meeting_name', {}).catch(() => null),
  ]);
  return {
    startedAt,
    finishedAt: new Date().toISOString(),
    requestedDurationMs: durationMs,
    pollMs,
    changes,
    stateSamples,
    finalHistory,
    finalState,
    finalFolder,
    finalMeetingName,
    href: location.href,
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
fs.mkdirSync(path.dirname(outputPath), { recursive: true });
fs.writeFileSync(outputPath, `${JSON.stringify(result, null, 2)}\n`, "utf8");
console.log(
  JSON.stringify(
    {
      outputPath: path.resolve(outputPath),
      startedAt: result.startedAt,
      finishedAt: result.finishedAt,
      changeCount: result.changes.length,
      finalSegmentCount: Array.isArray(result.finalHistory) ? result.finalHistory.length : null,
      finalState: result.finalState,
      finalFolder: result.finalFolder,
    },
    null,
    2,
  ),
);
socket.close();
