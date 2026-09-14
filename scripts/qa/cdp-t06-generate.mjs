import fs from "node:fs";

const meetingId = process.argv[2];
const timeoutMs = Math.max(1000, Number.parseInt(process.argv[3] ?? "180000", 10));
const outputPath = process.argv[4];
if (!meetingId) throw new Error("Usage: node cdp-t06-generate.mjs <meeting-id> [timeout-ms] [output.json]");
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
  const timeoutMs = ${JSON.stringify(timeoutMs)};
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const historyBefore = await invoke('api_list_summary_generation_history', { meetingId });
  const meetingBefore = await invoke('api_get_meeting', { meetingId });
  const button = [...document.querySelectorAll('button')].find(
    (candidate) => {
      const text = candidate.innerText.trim();
      return !candidate.disabled && (
        text === '生成摘要'
        || text === '重新生成摘要'
        || candidate.title === '生成 AI 摘要'
        || candidate.title === '重新生成 AI 摘要'
      );
    }
  );
  if (!button) throw new Error('Enabled generate or regenerate summary button was not found');

  const startedAt = new Date().toISOString();
  const startedPerformance = performance.now();
  const observations = [];
  let priorKey = '';
  let lastHistory = historyBefore;
  let lastHistoryPoll = -1000;
  button.click();
  let confirmation = null;
  const confirmationStarted = performance.now();
  while (performance.now() - confirmationStarted < 3000) {
    const currentTemplateButton = [...document.querySelectorAll('button')].find(
      (candidate) => candidate.innerText.includes('使用当前最新模板') && !candidate.disabled
    );
    if (currentTemplateButton) {
      confirmation = {
        shownAt: new Date().toISOString(),
        elapsedMs: performance.now() - startedPerformance,
        text: currentTemplateButton.innerText,
      };
      currentTemplateButton.click();
      confirmation.confirmedAt = new Date().toISOString();
      break;
    }
    const alreadyGenerating = (document.body?.innerText ?? '').includes('正在生成摘要');
    if (alreadyGenerating) break;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }

  while (performance.now() - startedPerformance < timeoutMs) {
    const elapsedMs = performance.now() - startedPerformance;
    const text = document.body?.innerText ?? '';
    if (elapsedMs - lastHistoryPoll >= 1000) {
      lastHistory = await invoke('api_list_summary_generation_history', { meetingId });
      lastHistoryPoll = elapsedMs;
    }
    const newest = lastHistory[0] ?? null;
    const generating = text.includes('正在生成 AI 摘要')
      || text.includes('正在重新生成摘要')
      || [...document.querySelectorAll('button')].some((candidate) => (
        candidate.title === '停止生成摘要'
        || candidate.getAttribute('aria-busy') === 'true'
      ));
    const errorCardCount = [...document.querySelectorAll('[role="alert"]')].filter((item) => {
      const rect = item.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && /摘要|模型|API|失败|错误/.test(item.innerText);
    }).length;
    const errorVisible = text.includes('生成摘要时出错')
      || text.includes('无法完成此操作')
      || (newest?.status === 'failed' && lastHistory.length === historyBefore.length + 1);
    const historyDelta = lastHistory.length - historyBefore.length;
    const complete = !generating
      && !errorVisible
      && historyDelta === 1
      && newest?.status === 'completed';
    const key = JSON.stringify([
      generating,
      errorVisible,
      errorCardCount,
      lastHistory.length,
      newest?.status ?? null,
      complete,
    ]);
    if (key !== priorKey) {
      observations.push({
        at: new Date().toISOString(),
        elapsedMs,
        generating,
        errorVisible,
        errorCardCount,
        historyCount: lastHistory.length,
        historyDelta,
        newestStatus: newest?.status ?? null,
        complete,
        generateButtons: [...document.querySelectorAll('button')]
          .filter((candidate) => /生成.*摘要/.test(candidate.innerText) || /生成.*摘要/.test(candidate.title))
          .map((candidate) => ({ text: candidate.innerText.trim(), title: candidate.title, disabled: candidate.disabled, ariaBusy: candidate.getAttribute('aria-busy') })),
        textTail: text.slice(-1800),
      });
      priorKey = key;
    }
    if (complete || errorVisible) break;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  const finishedAt = new Date().toISOString();
  const historyAfter = await invoke('api_list_summary_generation_history', { meetingId });
  const meetingAfter = await invoke('api_get_meeting', { meetingId });
  const finalText = document.body?.innerText ?? '';
  const visibleAlerts = [...document.querySelectorAll('[role="alert"]')].filter((item) => {
    const rect = item.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }).map((item) => item.innerText);
  const factReviewCards = visibleAlerts.filter((text) => (
    text.includes('需要核对事实')
    || text.includes('事实校验需要检查')
    || text.includes('needs a fact check')
  ));
  const errorCards = visibleAlerts.filter((text) => (
    text.includes('生成摘要时出错')
    || text.includes('无法完成此操作')
    || /(?:模型|API).*(?:失败|错误)/.test(text)
    || /(?:model|API).*(?:failed|error)/i.test(text)
  ));
  return {
    meetingId,
    startedAt,
    finishedAt,
    elapsedMs: performance.now() - startedPerformance,
    timeoutMs,
    href: location.href,
    historyBefore,
    historyAfter,
    newest: historyAfter[0] ?? null,
    meetingBefore,
    meetingAfter,
    confirmation,
    observations,
    visibleAlerts,
    factReviewCards,
    errorCards,
    finalText: finalText.slice(-12000),
    verdict: {
      completedWithinLimit: performance.now() - startedPerformance <= timeoutMs
        && historyAfter.length === historyBefore.length + 1
        && historyAfter[0]?.status === 'completed',
      exactlyOneNewHistory: historyAfter.length === historyBefore.length + 1,
      noErrorVisible: !finalText.includes('生成摘要时出错') && !finalText.includes('无法完成此操作'),
      noVisibleErrorCards: errorCards.length === 0,
      factReviewVisible: factReviewCards.length > 0,
      urlUnchanged: location.href.includes(meetingId),
      summaryVisibleAndNonEmpty: /摘要|会议结论|关键决策/.test(finalText)
        && finalText.length > 200,
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
