import fs from "node:fs";

const outputPath = process.argv[2];
if (!outputPath) throw new Error("Usage: node cdp-capture-page.mjs <output.png>");
const cdpPort = process.env.CDP_PORT ?? "9233";
const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`).then((response) =>
  response.json(),
);
const page = targets.find(
  (target) => target.type === "page" && (
    target.url.startsWith("http://tauri.localhost") ||
    target.url.startsWith("http://localhost:")
  ),
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
await call("Page.enable");
let signal = null;
if (process.argv[3] === "--wait-for-signal") {
  const observed = await call("Runtime.evaluate", {
    expression: `(async () => {
      const deadline = performance.now() + 30000;
      while (performance.now() < deadline) {
        const state = await window.__TAURI_INTERNALS__.invoke('get_recording_state', {});
        if (state.is_recording && state.system_route.rms_level > 0.005 &&
            document.body.innerText.includes('正在收到声音')) {
          return { at: new Date().toISOString(), state, visibleText: document.body.innerText };
        }
        await new Promise(resolve => setTimeout(resolve, 50));
      }
      throw new Error('No simultaneous real system signal and visible receiving indicator');
    })()`,
    returnByValue: true, awaitPromise: true,
  });
  if (observed.exceptionDetails) {
    socket.close();
    throw new Error(JSON.stringify(observed.exceptionDetails));
  }
  signal = observed.result.value;
}
const result = await call("Page.captureScreenshot", { format: "png" });
fs.writeFileSync(outputPath, Buffer.from(result.data, "base64"));
console.log(JSON.stringify({ capturedAt: new Date().toISOString(), outputPath, pageUrl: page.url, signal }, null, 2));
socket.close();
