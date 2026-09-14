const meetingId = process.argv[2];
if (!meetingId) throw new Error("Usage: node cdp-t07-restore-manual.mjs <meeting-id>");

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
function call(method, params = {}) {
  const id = nextId++;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

await call("Runtime.enable");
const expression = `(async () => {
  const meetingId = ${JSON.stringify(meetingId)};
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const isVisible = (element) => {
    const rect = element.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const historyBefore = await invoke('api_list_summary_generation_history', { meetingId });
  const manualBefore = await invoke('api_list_manual_summary_revisions', { meetingId });
  const summaryBefore = await invoke('api_get_summary', { meetingId });
  const bodyBefore = document.body?.innerText ?? '';
  const historyButton = [...document.querySelectorAll('button')].find(
    (button) => (button.title === '摘要生成历史' || button.innerText.trim() === '历史')
      && !button.disabled && isVisible(button)
  );
  if (!historyButton) throw new Error('Summary generation history button was not found');
  historyButton.click();

  let restoreButton = null;
  const dialogDeadline = performance.now() + 5000;
  while (performance.now() < dialogDeadline) {
    restoreButton = [...document.querySelectorAll('button')].find(
      (button) => button.innerText.includes('恢复此版本') && !button.disabled && isVisible(button)
    );
    if (restoreButton) break;
    await sleep(100);
  }
  if (!restoreButton) throw new Error('Enabled manual revision restore button was not found');
  const clickedAt = new Date().toISOString();
  restoreButton.click();

  let toastSeen = false;
  let markerVisible = false;
  const restoreDeadline = performance.now() + 6000;
  while (performance.now() < restoreDeadline) {
    const body = document.body?.innerText ?? '';
    toastSeen ||= body.includes('已恢复人工保存版本');
    markerVisible ||= body.includes('【摘要人工校正】');
    if (toastSeen && markerVisible && !body.includes('摘要生成历史')) break;
    await sleep(100);
  }

  const historyAfter = await invoke('api_list_summary_generation_history', { meetingId });
  const manualAfter = await invoke('api_list_manual_summary_revisions', { meetingId });
  const summaryAfter = await invoke('api_get_summary', { meetingId });
  const bodyAfter = document.body?.innerText ?? '';
  return {
    meetingId,
    clickedAt,
    finishedAt: new Date().toISOString(),
    bodyBeforeContainsMarker: bodyBefore.includes('【摘要人工校正】'),
    bodyAfterContainsMarker: bodyAfter.includes('【摘要人工校正】'),
    toastSeen,
    historyBefore,
    historyAfter,
    manualBefore,
    manualAfter,
    summaryChanged: JSON.stringify(summaryBefore ?? null) !== JSON.stringify(summaryAfter ?? null),
    finalTextHead: bodyAfter.slice(0, 5000),
    verdict: {
      priorGeneratedSummaryHadNoMarker: !bodyBefore.includes('【摘要人工校正】'),
      restoreSuccessSurfaced: toastSeen,
      restoredMarkerVisible: bodyAfter.includes('【摘要人工校正】'),
      generationHistoryUnchanged: historyAfter.length === historyBefore.length,
      manualHistoryUnchanged: manualAfter.length === manualBefore.length,
      manualRevisionNowCurrent: manualAfter.some((revision) => revision.isCurrent && revision.markdown?.includes('【摘要人工校正】')),
      sourceGenerationNowCurrent: historyAfter.some((item) => (
        item.generationId === manualBefore[0]?.sourceGenerationId && item.isCurrentSummary
      )),
      summaryActuallyChanged: JSON.stringify(summaryBefore ?? null) !== JSON.stringify(summaryAfter ?? null),
    },
  };
})()`;
const evaluated = await call("Runtime.evaluate", {
  expression,
  returnByValue: true,
  awaitPromise: true,
  userGesture: true,
});
if (evaluated.exceptionDetails) {
  throw new Error(evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text);
}
console.log(JSON.stringify(evaluated.result.value, null, 2));
socket.close();
