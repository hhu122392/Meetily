import fs from "node:fs";

const outputPath = process.argv[2];
const cdpPort = process.env.CDP_PORT;
const cdpTargetId = process.env.CDP_TARGET_ID;
if (!/^\d+$/.test(cdpPort ?? "") || !cdpTargetId) {
  throw new Error("CDP_PORT and the process-bound CDP_TARGET_ID are required");
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
socket.addEventListener("close", () => rejectPending(new Error("CDP socket closed before the exit request completed")));
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

try {
  const response = await call("Runtime.evaluate", {
    expression: `(() => {
      if (!window.__TAURI_INTERNALS__?.invoke) throw new Error('Tauri invoke is not ready');
      const requestedAt = new Date().toISOString();
      void window.__TAURI_INTERNALS__.invoke('plugin:process|exit', { code: 0 });
      return { requestedAt, method: 'plugin:process|exit', code: 0 };
    })()`,
    returnByValue: true,
    userGesture: true,
  });
  if (response.exceptionDetails) {
    throw new Error(response.exceptionDetails.exception?.description ?? response.exceptionDetails.text);
  }
  const record = {
    ...response.result.value,
    cdpTargetId,
    cdpTargetUrl: page.url,
  };
  const output = `${JSON.stringify(record, null, 2)}\n`;
  if (outputPath) fs.writeFileSync(outputPath, output, "utf8");
  console.log(output.trimEnd());
} finally {
  socket.close();
}
