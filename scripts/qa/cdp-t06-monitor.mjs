import fs from "node:fs";

const meetingId = process.argv[2];
const clickStartedAt = process.argv[3];
const historyCountBefore = Number.parseInt(process.argv[4] ?? "1", 10);
const timeoutMs = Math.max(1000, Number.parseInt(process.argv[5] ?? "180000", 10));
const outputPath = process.argv[6];
if (!meetingId || !clickStartedAt) {
  throw new Error(
    "Usage: node cdp-t06-monitor.mjs <meeting-id> <click-started-at> [history-before] [timeout-ms]",
  );
}
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

await call("Runtime.enable");
const expression = `(async () => {
  const meetingId = ${JSON.stringify(meetingId)};
  const clickStartedAt = ${JSON.stringify(clickStartedAt)};
  const historyCountBefore = ${JSON.stringify(historyCountBefore)};
  const timeoutMs = ${JSON.stringify(timeoutMs)};
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const observations = [];
  let priorKey = '';
  const initialElapsedMs = Date.now() - Date.parse(clickStartedAt);
  const remainingMs = Math.max(0, timeoutMs - initialElapsedMs);
  const monitorStarted = performance.now();
  let history = [];

  while (performance.now() - monitorStarted <= remainingMs) {
    history = await invoke('api_list_summary_generation_history', { meetingId });
    const newest = history[0] ?? null;
    const text = document.body?.innerText ?? '';
    const stopButtonVisible = [...document.querySelectorAll('button')].some(
      (button) => button.title === '停止生成摘要' && !button.disabled
    );
    const generating = stopButtonVisible
      || text.includes('正在生成 AI 摘要')
      || text.includes('正在重新生成摘要')
      || newest?.status === 'pending'
      || newest?.status === 'processing';
    const errorVisible = text.includes('生成摘要时出错') || text.includes('无法完成此操作');
    const completed = history.length === historyCountBefore + 1 && newest?.status === 'completed' && !generating;
    const failed = newest?.status === 'failed' || errorVisible;
    const key = JSON.stringify([history.length, newest?.status, generating, errorVisible, completed, failed]);
    if (key !== priorKey) {
      observations.push({
        at: new Date().toISOString(),
        totalElapsedMs: Date.now() - Date.parse(clickStartedAt),
        historyCount: history.length,
        newestStatus: newest?.status ?? null,
        newestGenerationId: newest?.generationId ?? null,
        generating,
        stopButtonVisible,
        errorVisible,
        completed,
        failed,
        textTail: text.slice(-2200),
      });
      priorKey = key;
    }
    if (completed || failed) break;
    await new Promise((resolve) => setTimeout(resolve, 500));
  }

  history = await invoke('api_list_summary_generation_history', { meetingId });
  const newest = history[0] ?? null;
  const finalText = document.body?.innerText ?? '';
  const errorCards = [...document.querySelectorAll('[role="alert"]')].filter((item) => {
    const rect = item.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }).map((item) => item.innerText);
  const totalElapsedMs = Date.now() - Date.parse(clickStartedAt);
  return {
    meetingId,
    clickStartedAt,
    monitoredAt: new Date().toISOString(),
    totalElapsedMs,
    timeoutMs,
    historyCountBefore,
    history,
    newest,
    observations,
    errorCards,
    href: location.href,
    finalText: finalText.slice(-15000),
    verdict: {
      completedWithinLimit: newest?.status === 'completed' && totalElapsedMs <= timeoutMs,
      exactlyOneNewHistory: history.length === historyCountBefore + 1,
      noErrorVisible: !finalText.includes('生成摘要时出错') && !finalText.includes('无法完成此操作'),
      noVisibleErrorCards: errorCards.length === 0,
      taskNotLeftRunning: newest?.status !== 'pending' && newest?.status !== 'processing'
        && !finalText.includes('正在生成 AI 摘要') && !finalText.includes('正在重新生成摘要'),
      urlUnchanged: location.href.includes(meetingId),
      summaryVisibleAndNonEmpty: /摘要|会议结论|关键决策/.test(finalText) && finalText.length > 200,
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
const output = `${JSON.stringify(evaluated.result.value, null, 2)}\n`;
if (outputPath) fs.writeFileSync(outputPath, output, "utf8");
console.log(output.trimEnd());
socket.close();
