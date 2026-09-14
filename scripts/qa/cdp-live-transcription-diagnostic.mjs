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
  const invoke = window.__TAURI_INTERNALS__?.invoke;
  if (typeof invoke !== "function") {
    throw new Error("Tauri invoke bridge is unavailable");
  }
  const safeInvoke = async (command, payload = {}) => {
    const started = performance.now();
    try {
      const result = await invoke(command, payload);
      return { ok: true, elapsedMs: performance.now() - started, result };
    } catch (error) {
      return { ok: false, elapsedMs: performance.now() - started, error: String(error) };
    }
  };

  const [config, preferences, devices, recordingState, meetingName, history] =
    await Promise.all([
      safeInvoke("api_get_transcript_config"),
      safeInvoke("get_recording_preferences"),
      safeInvoke("get_audio_devices"),
      safeInvoke("get_recording_state"),
      safeInvoke("get_recording_meeting_name"),
      safeInvoke("get_transcript_history"),
    ]);

  if (config.ok && config.result && typeof config.result === "object") {
    config.result = {
      provider: config.result.provider,
      model: config.result.model,
      hasApiKey: Boolean(config.result.api_key),
    };
  }

  return {
    capturedAt: new Date().toISOString(),
    href: location.href,
    documentLanguage: document.documentElement.lang,
    localStorage: {
      primaryLanguage: localStorage.getItem("primaryLanguage"),
      uiLocalePreference: localStorage.getItem("uiLocalePreference"),
    },
    config,
    preferences,
    devices,
    recordingState,
    meetingName,
    history,
    visibleText: document.body?.innerText?.slice(0, 16000) ?? "",
  };
})()`;
const evaluated = await call("Runtime.evaluate", {
  expression,
  returnByValue: true,
  awaitPromise: true,
  userGesture: true,
});
if (evaluated.exceptionDetails) {
  throw new Error(evaluated.exceptionDetails.text ?? "Runtime.evaluate failed");
}
console.log(JSON.stringify(evaluated.result.value, null, 2));
socket.close();
