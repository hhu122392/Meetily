const meetingId = process.argv[2];
const marker = process.argv[3] ?? "【未保存摘要】";
const timeoutMs = Math.max(1000, Number.parseInt(process.argv[4] ?? "180000", 10));
if (!meetingId) throw new Error("Usage: node cdp-t07-save-continue-regenerate.mjs <meeting-id> [marker] [timeout-ms]");
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
await call("Runtime.enable");
const expression = `(async () => {
  const meetingId = ${JSON.stringify(meetingId)};
  const marker = ${JSON.stringify(marker)};
  const timeoutMs = ${JSON.stringify(timeoutMs)};
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const historyBefore = await invoke('api_list_summary_generation_history', { meetingId });
  const backendBefore = await invoke('api_get_summary', { meetingId });
  const editor = [...document.querySelectorAll('[contenteditable="true"]')]
    .find((element) => element.getBoundingClientRect().width > 300 && element.innerText.includes(marker));
  if (!editor) throw new Error('Dirty editor with marker was not found');
  const regenerateButton = [...document.querySelectorAll('button')].find((button) =>
    button.title === '重新生成 AI 摘要' && !button.disabled
  );
  if (!regenerateButton) throw new Error('Regenerate button was not found');
  const startedAt = new Date().toISOString();
  const startedPerformance = performance.now();
  regenerateButton.click();

  let guardText = null;
  let saveAndContinueButton = null;
  while (performance.now() - startedPerformance < 5000) {
    saveAndContinueButton = [...document.querySelectorAll('button')].find((button) =>
      button.innerText.trim() === '保存后继续' && !button.disabled
    );
    if (saveAndContinueButton) {
      guardText = saveAndContinueButton.closest('[role="dialog"]')?.innerText ?? null;
      break;
    }
    await delay(50);
  }
  if (!saveAndContinueButton) throw new Error('Unsaved-change guard was not shown');
  saveAndContinueButton.click();

  let backendSavedBeforeGeneration = null;
  let latestTemplateButton = null;
  while (performance.now() - startedPerformance < 15000) {
    backendSavedBeforeGeneration = await invoke('api_get_summary', { meetingId });
    latestTemplateButton = [...document.querySelectorAll('button')].find((button) =>
      button.innerText.includes('使用当前最新模板') && !button.disabled
    );
    if (JSON.stringify(backendSavedBeforeGeneration).includes(marker) && latestTemplateButton) break;
    await delay(100);
  }
  if (!JSON.stringify(backendSavedBeforeGeneration).includes(marker)) {
    throw new Error('Save-and-continue did not persist marker before regeneration');
  }
  if (!latestTemplateButton) throw new Error('Template-version chooser did not open after saving');
  const templateDialogText = latestTemplateButton.closest('[role="dialog"]')?.innerText ?? null;
  latestTemplateButton.click();

  const observations = [];
  let priorKey = '';
  let history = historyBefore;
  while (performance.now() - startedPerformance < timeoutMs) {
    history = await invoke('api_list_summary_generation_history', { meetingId });
    const newest = history[0] ?? null;
    const text = document.body?.innerText ?? '';
    const generating = text.includes('正在生成 AI 摘要')
      || text.includes('正在重新生成摘要')
      || [...document.querySelectorAll('button')].some((button) => button.title === '停止生成摘要');
    const errorVisible = text.includes('生成摘要时出错') || text.includes('无法完成此操作');
    const complete = history.length === historyBefore.length + 1
      && newest?.status === 'completed'
      && !generating
      && !errorVisible;
    const key = JSON.stringify([history.length, newest?.status, generating, errorVisible, complete]);
    if (key !== priorKey) {
      observations.push({
        at: new Date().toISOString(),
        elapsedMs: performance.now() - startedPerformance,
        historyCount: history.length,
        newestStatus: newest?.status ?? null,
        generating,
        errorVisible,
        complete,
        textTail: text.slice(-1800),
      });
      priorKey = key;
    }
    if (complete || errorVisible || newest?.status === 'failed') break;
    await delay(500);
  }
  const historyAfter = await invoke('api_list_summary_generation_history', { meetingId });
  const finalSummary = await invoke('api_get_summary', { meetingId });
  const finalText = document.body?.innerText ?? '';
  return {
    meetingId,
    marker,
    startedAt,
    finishedAt: new Date().toISOString(),
    elapsedMs: performance.now() - startedPerformance,
    guardText,
    templateDialogText,
    historyBefore,
    historyAfter,
    backendBefore,
    backendSavedBeforeGeneration,
    finalSummary,
    finalText: finalText.slice(-12000),
    observations,
    verdict: {
      guardShown: Boolean(guardText?.includes('未保存')),
      savedMarkerBeforeRegeneration: JSON.stringify(backendSavedBeforeGeneration).includes(marker),
      exactlyOneNewGeneration: historyAfter.length === historyBefore.length + 1,
      newestCompleted: historyAfter[0]?.status === 'completed',
      completedWithinLimit: performance.now() - startedPerformance <= timeoutMs,
      noErrorVisible: !finalText.includes('生成摘要时出错') && !finalText.includes('无法完成此操作'),
    },
  };
})()`;
const evaluated = await call("Runtime.evaluate", {
  expression,
  returnByValue: true,
  awaitPromise: true,
  userGesture: true,
});
if (evaluated.exceptionDetails) throw new Error(evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text);
console.log(JSON.stringify(evaluated.result.value, null, 2));
socket.close();
