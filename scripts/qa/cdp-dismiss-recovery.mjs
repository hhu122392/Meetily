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

const expression = `(() => {
  const bodyText = document.body?.innerText ?? '';
  if (!bodyText.includes('恢复中断的会议')) {
    return { dismissed: false, reason: 'not_present' };
  }
  const cancel = [...document.querySelectorAll('button')].find((button) => {
    const rect = button.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0 && button.innerText.trim() === '取消';
  });
  if (!cancel) return { dismissed: false, reason: 'cancel_not_found' };
  cancel.click();
  return { dismissed: true, reason: 'cancel_clicked' };
})()`;
const evaluated = await call("Runtime.evaluate", { expression, returnByValue: true });
if (evaluated.exceptionDetails) {
  throw new Error(evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text);
}
console.log(JSON.stringify(evaluated.result.value, null, 2));
socket.close();
