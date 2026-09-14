const [stopAt, expectedFolder] = process.argv.slice(2);
const stopAtMs = stopAt ? Date.parse(stopAt) : null;
if (stopAt && (!Number.isFinite(stopAtMs) || !expectedFolder || !process.env.CDP_TARGET_ID)) {
  throw new Error("Timed stop requires an ISO time, expected folder and process-bound CDP_TARGET_ID");
}
const cdpPort = process.env.CDP_PORT ?? "9233";
const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`).then((response) => response.json());
const page = targets.find(
  (target) => target.type === "page" && target.url.startsWith("http://tauri.localhost")
    && (!process.env.CDP_TARGET_ID || target.id === process.env.CDP_TARGET_ID),
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
  const stopAtMs = ${JSON.stringify(stopAtMs)};
  const expectedFolder = ${JSON.stringify(expectedFolder ?? null)};
  if (stopAtMs !== null) {
    if (Date.now() >= stopAtMs || stopAtMs - Date.now() > 180000) throw new Error('Timed stop must be armed before its deadline, within three minutes');
    await sleep(stopAtMs - Date.now());
    if (Date.now() - stopAtMs > 1000) throw new Error('Timed stop missed its deadline; not stopping an unverified later session');
  }
  const before = await invoke('get_recording_state', {});
  const folderBefore = await invoke('get_meeting_folder_path', {});
  if (expectedFolder !== null && (folderBefore !== expectedFolder || !before.is_recording || !before.is_active)) {
    throw new Error('Timed stop recording identity changed');
  }
  const stopButtons = [...document.querySelectorAll('button')].filter((button) => {
    const rect = button.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0
      && (button.getAttribute('aria-label') ?? '').includes('停止录音')
      && !button.disabled;
  });
  if (stopButtons.length !== 1) throw new Error('Expected exactly one enabled stop-recording button');
  const clickedAt = new Date().toISOString();
  stopButtons[0].click();
  const samples = [];
  const deadline = performance.now() + 20000;
  while (performance.now() < deadline) {
    const state = await invoke('get_recording_state', {});
    samples.push({ at: new Date().toISOString(), state, href: location.href });
    if (!state.is_recording && !state.is_active) break;
    await sleep(200);
  }
  const after = await invoke('get_recording_state', {});
  return {
    targetStopAt: stopAtMs === null ? null : new Date(stopAtMs).toISOString(),
    clickedAt,
    stopButtonCount: stopButtons.length,
    folderBefore,
    before,
    after,
    samples,
    verdict: {
      oneStopClick: stopButtons.length === 1,
      wasActuallyRecording: before.is_recording === true && before.is_active === true,
      backendStopped: after.is_recording === false && after.is_active === false,
    },
  };
})()`;
const evaluated = await call("Runtime.evaluate", {
  expression,
  returnByValue: true,
  awaitPromise: true,
  userGesture: true,
});
if (evaluated.exceptionDetails) {
  throw new Error(evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text);
}
console.log(JSON.stringify(evaluated.result.value, null, 2));
socket.close();
