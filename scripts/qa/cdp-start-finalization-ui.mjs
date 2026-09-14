import fs from "node:fs";

const specificationPath = process.argv[2];
const outputPath = process.argv[3];
if (!specificationPath) {
  throw new Error("Usage: node cdp-start-finalization-ui.mjs <specification.json>");
}
const specification = JSON.parse(fs.readFileSync(specificationPath, "utf8"));
const meetingId = specification.payload?.meetingId;
if (!meetingId || !specification.command) throw new Error("Invalid finalization specification");

const cdpPort = process.env.CDP_PORT ?? "9222";
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
await call("Page.enable");
const storageKey = `meetily_recording_finalization_${meetingId}`;
const destination = `/meeting-details?id=${encodeURIComponent(meetingId)}&source=recording-finalizing`;
const expression = `(() => {
  const command = ${JSON.stringify(specification.command)};
  const payload = ${JSON.stringify(specification.payload)};
  const storageKey = ${JSON.stringify(storageKey)};
  const destination = ${JSON.stringify(destination)};
  const startedAt = new Date().toISOString();
  const startedPerformance = performance.now();
  sessionStorage.setItem(storageKey, 'pending');
  window.__meetilyQaFinalization = { startedAt, status: 'pending' };
  void window.__TAURI_INTERNALS__.invoke(command, payload).then((result) => {
    sessionStorage.setItem(storageKey, 'completed');
    window.__meetilyQaFinalization = {
      startedAt,
      finishedAt: new Date().toISOString(),
      elapsedMs: performance.now() - startedPerformance,
      status: 'completed',
      result,
    };
  }).catch((error) => {
    sessionStorage.setItem(storageKey, 'failed');
    window.__meetilyQaFinalization = {
      startedAt,
      finishedAt: new Date().toISOString(),
      elapsedMs: performance.now() - startedPerformance,
      status: 'failed',
      error: String(error),
    };
  });
  return { command, meetingId: payload.meetingId, storageKey, destination, startedAt };
})()`;
const evaluated = await call("Runtime.evaluate", {
  expression,
  returnByValue: true,
  awaitPromise: false,
  userGesture: true,
});
if (evaluated.exceptionDetails) throw new Error(evaluated.exceptionDetails.text);
await call("Page.navigate", {
  url: new URL(destination, page.url).href,
});
const output = `${JSON.stringify(evaluated.result.value, null, 2)}\n`;
if (outputPath) fs.writeFileSync(outputPath, output, "utf8");
console.log(output.trimEnd());
socket.close();
