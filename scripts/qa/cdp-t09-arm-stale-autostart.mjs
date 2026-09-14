const cdpPort = process.env.CDP_PORT ?? "9233";
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
const requestId = nextId++;
socket.send(JSON.stringify({
  id: requestId,
  method: "Runtime.evaluate",
  params: {
    expression: `(async () => {
      const appSessionId = await window.__TAURI_INTERNALS__.invoke('get_app_session_id', {});
      const request = {
        version: 3,
        requestedAt: Date.now(),
        rendererSessionId: 'qa-renderer-before-restart',
        appSessionId
      };
      sessionStorage.setItem('autoStartRecording', JSON.stringify(request));
      return {
        armedAt: new Date().toISOString(),
        href: location.href,
        request,
        storedValue: sessionStorage.getItem('autoStartRecording')
      };
    })()`,
    returnByValue: true,
    awaitPromise: true,
    userGesture: true,
  },
}));
const evaluated = await new Promise((resolve, reject) => pending.set(requestId, { resolve, reject }));
if (evaluated.exceptionDetails) throw new Error(evaluated.exceptionDetails.text);
console.log(JSON.stringify(evaluated.result.value, null, 2));
socket.close();
