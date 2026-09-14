const meetingId = process.argv[2];
if (!meetingId) throw new Error("Usage: node cdp-navigate-meeting.mjs <meeting-id>");

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

await call("Page.enable");
const destination = `http://tauri.localhost/meeting-details?id=${encodeURIComponent(meetingId)}&source=recording`;
await call("Page.navigate", { url: destination });

const startedAt = Date.now();
let state = null;
while (Date.now() - startedAt < 30000) {
  const evaluated = await call("Runtime.evaluate", {
    expression: `({ href: location.href, text: (document.body?.innerText ?? '').slice(0, 3000) })`,
    returnByValue: true,
  });
  state = evaluated.result.value;
  if (state.href.includes(meetingId) && state.text.length > 100) break;
  await new Promise((resolve) => setTimeout(resolve, 250));
}

console.log(JSON.stringify({
  navigatedAt: new Date().toISOString(),
  destination,
  href: state?.href ?? null,
  targetMeetingVisible: Boolean(state?.href?.includes(meetingId)),
  textHead: state?.text ?? "",
}, null, 2));
socket.close();
