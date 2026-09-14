const cdpPort = process.env.CDP_PORT ?? "9233";
const timeoutMs = Number.parseInt(process.argv[2] ?? "12000", 10);
const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`).then((response) => response.json());
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
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const appSessionId = await invoke('get_app_session_id', {});
  const samples = [];
  const deadline = performance.now() + ${JSON.stringify(timeoutMs)};
  while (performance.now() < deadline) {
    const state = await invoke('get_recording_state', {});
    const transcription = await invoke('get_transcription_status', {});
    samples.push({
      at: new Date().toISOString(),
      href: location.href,
      state,
      transcription,
      pendingAutoStartValue: sessionStorage.getItem('autoStartRecording'),
      bodyRecording: (document.body?.innerText ?? '').includes('正在录音'),
    });
    await sleep(250);
  }
  const final = samples[samples.length - 1];
  return {
    capturedAt: new Date().toISOString(),
    durationMs: ${JSON.stringify(timeoutMs)},
    appSessionId,
    samples,
    verdict: {
      neverRecorded: samples.every((sample) => !sample.state.is_recording && !sample.state.is_active),
      neverTranscribed: samples.every((sample) => !sample.transcription.is_processing && sample.transcription.chunks_in_queue === 0),
      staleRequestCleared: final.pendingAutoStartValue === null,
      noRecordingUi: samples.every((sample) => !sample.bodyRecording),
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
console.log(JSON.stringify(evaluated.result.value, null, 2));
socket.close();
