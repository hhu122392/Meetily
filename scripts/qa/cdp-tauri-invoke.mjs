import fs from "node:fs";

const specificationPath = process.argv[2];
const outputPath = process.argv[3];
if (!specificationPath) {
  throw new Error("Usage: node cdp-tauri-invoke.mjs <specification.json> [output.json]");
}

const specification = JSON.parse(fs.readFileSync(specificationPath, "utf8"));
if (!specification.command || typeof specification.payload !== "object") {
  throw new Error("The specification must contain command and payload fields");
}

const cdpPort = process.env.CDP_PORT ?? "9222";
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

await call("Runtime.enable");
const expression = `(async () => {
  const command = ${JSON.stringify(specification.command)};
  const payload = ${JSON.stringify(specification.payload)};
  const startedAt = new Date().toISOString();
  const startedPerformance = performance.now();
  try {
    const result = await window.__TAURI_INTERNALS__.invoke(command, payload);
    return {
      command,
      startedAt,
      finishedAt: new Date().toISOString(),
      elapsedMs: performance.now() - startedPerformance,
      ok: true,
      result,
    };
  } catch (error) {
    return {
      command,
      startedAt,
      finishedAt: new Date().toISOString(),
      elapsedMs: performance.now() - startedPerformance,
      ok: false,
      error: String(error),
    };
  }
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
const output = `${JSON.stringify(evaluated.result.value, null, 2)}\n`;
if (outputPath) {
  fs.writeFileSync(outputPath, output, "utf8");
}
console.log(output.trimEnd());
socket.close();
