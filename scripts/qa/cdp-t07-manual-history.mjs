const meetingId = process.argv[2];
if (!meetingId) throw new Error("Usage: node cdp-t07-manual-history.mjs <meeting-id>");
const cdpPort = process.env.CDP_PORT ?? "9233";
const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`).then((response) => response.json());
const page = targets.find((target) => target.type === "page" && target.url.startsWith("http://tauri.localhost"));
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
const expression = `(async () => {
  const meetingId = ${JSON.stringify(meetingId)};
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const manualRevisions = await invoke('api_list_manual_summary_revisions', { meetingId });
  const generationHistory = await invoke('api_list_summary_generation_history', { meetingId });
  const historyButton = [...document.querySelectorAll('button')].find((button) =>
    button.getAttribute('aria-label') === '摘要生成历史' && !button.disabled
  );
  if (!historyButton) throw new Error('History button was not found');
  historyButton.click();
  let dialog = null;
  const started = performance.now();
  while (performance.now() - started < 10000) {
    dialog = [...document.querySelectorAll('[role="dialog"]')].find((item) =>
      item.getBoundingClientRect().width > 0 && item.innerText.includes('人工保存版本')
    );
    if (dialog && (!manualRevisions.length || dialog.innerText.includes('【摘要人工校正】'))) break;
    await delay(100);
  }
  const dialogText = dialog?.innerText ?? null;
  return {
    capturedAt: new Date().toISOString(),
    meetingId,
    manualRevisions,
    generationHistory,
    dialogText,
    verdict: {
      manualRevisionCount: manualRevisions.length,
      newestManualContainsMarker: JSON.stringify(manualRevisions[0] ?? null).includes('【摘要人工校正】'),
      newestManualCurrent: manualRevisions[0]?.isCurrent === true,
      generationHistoryCount: generationHistory.length,
      uiShowsManualSection: Boolean(dialogText?.includes('人工保存版本')),
      uiShowsMarker: Boolean(dialogText?.includes('【摘要人工校正】')),
    },
  };
})()`;
const evaluated = await call("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true, userGesture: true });
if (evaluated.exceptionDetails) throw new Error(evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text);
console.log(JSON.stringify(evaluated.result.value, null, 2));
socket.close();
