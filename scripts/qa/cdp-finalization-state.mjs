import fs from "node:fs";

const cdpPort = process.env.CDP_PORT ?? "9222";
const meetingId = process.argv[2];
const outputPath = process.argv[3];
if (!meetingId) throw new Error("Usage: node cdp-finalization-state.mjs <meeting-id>");
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
const storageKey = `meetily_recording_finalization_${meetingId}`;
const result = await call("Runtime.evaluate", {
  expression: `(() => {
    const inertRoot = document.querySelector('[inert]');
    const focusTarget = inertRoot?.querySelector('button, input, textarea, [tabindex]') ?? null;
    focusTarget?.focus();
    return {
      capturedAt: new Date().toISOString(),
      href: location.href,
      bodyText: document.body?.innerText ?? '',
      storageOutcome: sessionStorage.getItem(${JSON.stringify(storageKey)}),
      qaFinalization: window.__meetilyQaFinalization ?? null,
      busy: [...document.querySelectorAll('[aria-busy]')].map((element) => ({
        tag: element.tagName,
        ariaBusy: element.getAttribute('aria-busy'),
        inert: element.inert,
        text: element.innerText?.slice(0, 300) ?? '',
      })),
      inert: [...document.querySelectorAll('[inert]')].map((element) => ({
        tag: element.tagName,
        inert: element.inert,
        ariaBusy: element.getAttribute('aria-busy'),
        buttonCount: element.querySelectorAll('button').length,
        inputCount: element.querySelectorAll('input, textarea').length,
      })),
      focusProbe: {
        targetFound: Boolean(focusTarget),
        focusEnteredInertSubtree: Boolean(focusTarget && document.activeElement === focusTarget),
      },
      activeElement: {
        tag: document.activeElement?.tagName ?? null,
        text: document.activeElement?.innerText ?? null,
      },
    };
  })()`,
  returnByValue: true,
  awaitPromise: true,
});
if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
const output = `${JSON.stringify(result.result.value, null, 2)}\n`;
if (outputPath) fs.writeFileSync(outputPath, output, "utf8");
console.log(output.trimEnd());
socket.close();
