const meetingId = process.argv[2];
if (!meetingId) throw new Error("Usage: node cdp-t08-controlled-failure.mjs <meeting-id>");

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
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const isVisible = (element) => {
    const rect = element.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const historyBefore = await invoke('api_list_summary_generation_history', { meetingId });
  const meetingBefore = await invoke('api_get_meeting', { meetingId });
  const manualBefore = await invoke('api_list_manual_summary_revisions', { meetingId });
  const modelConfig = await invoke('api_get_model_config', {});
  const button = [...document.querySelectorAll('button')].find(
    (candidate) => (
      candidate.innerText.trim() === '重新生成摘要'
      || candidate.title === '重新生成 AI 摘要'
    ) && !candidate.disabled
  );
  if (!button) throw new Error('Enabled regenerate summary button was not found');

  const startedAt = new Date().toISOString();
  button.click();
  let templateChooserCount = 0;
  let selectedCurrentTemplate = false;
  const chooserDeadline = performance.now() + 4000;
  while (performance.now() < chooserDeadline) {
    const currentTemplateButtons = [...document.querySelectorAll('button')].filter(
      (candidate) => candidate.innerText.includes('使用当前最新模板') && !candidate.disabled && isVisible(candidate)
    );
    if (currentTemplateButtons.length > 0) {
      templateChooserCount = currentTemplateButtons.length;
      currentTemplateButtons[0].click();
      selectedCurrentTemplate = true;
      break;
    }
    await sleep(50);
  }

  const observations = [];
  let maxRelevantToastCount = 0;
  let maxRelevantAlertCount = 0;
  let settingsDialogSeen = false;
  const deadline = performance.now() + 4500;
  while (performance.now() < deadline) {
    const visibleToasts = [...document.querySelectorAll('[data-sonner-toast]')]
      .filter(isVisible)
      .map((element) => element.innerText.trim())
      .filter((text) => /模型|下载|不可用|失败|错误/.test(text));
    const visibleAlerts = [...document.querySelectorAll('[role="alert"]')]
      .filter(isVisible)
      .map((element) => element.innerText.trim())
      .filter((text) => /模型|下载|不可用|失败|错误/.test(text));
    const bodyText = document.body?.innerText ?? '';
    maxRelevantToastCount = Math.max(maxRelevantToastCount, visibleToasts.length);
    maxRelevantAlertCount = Math.max(maxRelevantAlertCount, visibleAlerts.length);
    settingsDialogSeen ||= bodyText.includes('模型设置')
      || bodyText.includes('AI 模型')
      || bodyText.includes('下载模型');
    observations.push({
      at: new Date().toISOString(),
      relevantToasts: visibleToasts,
      relevantAlerts: visibleAlerts,
      settingsDialogSeen,
      generating: bodyText.includes('正在生成 AI 摘要') || bodyText.includes('正在重新生成摘要'),
    });
    if (visibleToasts.length > 0 && settingsDialogSeen) break;
    await sleep(100);
  }

  await sleep(300);
  const historyAfter = await invoke('api_list_summary_generation_history', { meetingId });
  const meetingAfter = await invoke('api_get_meeting', { meetingId });
  const manualAfter = await invoke('api_list_manual_summary_revisions', { meetingId });
  const bodyText = document.body?.innerText ?? '';
  const visibleToasts = [...document.querySelectorAll('[data-sonner-toast]')]
    .filter(isVisible)
    .map((element) => element.innerText.trim());
  const visibleAlerts = [...document.querySelectorAll('[role="alert"]')]
    .filter(isVisible)
    .map((element) => element.innerText.trim());
  const summaryBefore = JSON.stringify(meetingBefore?.summary ?? null);
  const summaryAfter = JSON.stringify(meetingAfter?.summary ?? null);
  const savedEditPreserved = summaryBefore === summaryAfter
    && bodyText.includes('【摘要人工校正】');
  return {
    meetingId,
    startedAt,
    finishedAt: new Date().toISOString(),
    modelConfig,
    templateChooserCount,
    selectedCurrentTemplate,
    historyCountBefore: historyBefore.length,
    historyCountAfter: historyAfter.length,
    manualRevisionCountBefore: manualBefore.length,
    manualRevisionCountAfter: manualAfter.length,
    summaryUnchanged: summaryBefore === summaryAfter,
    markerVisibleAfterFailure: bodyText.includes('【摘要人工校正】'),
    visibleToasts,
    visibleAlerts,
    maxRelevantToastCount,
    maxRelevantAlertCount,
    settingsDialogSeen,
    bodyTextTail: bodyText.slice(-8000),
    observations,
    verdict: {
      configuredUnavailableModel: modelConfig?.provider === 'builtin-ai' && modelConfig?.model === 'gemma3:1b',
      oneTemplateAction: templateChooserCount === 1 && selectedCurrentTemplate,
      exactlyOneSurfacedError: maxRelevantToastCount === 1,
      noGenerationHistoryPollution: historyAfter.length === historyBefore.length,
      noManualHistoryPollution: manualAfter.length === manualBefore.length,
      summaryNotOverwritten: summaryBefore === summaryAfter,
      savedEditPreserved,
      actionableModelSettingsOpened: settingsDialogSeen,
      notStuckGenerating: !bodyText.includes('正在生成 AI 摘要') && !bodyText.includes('正在重新生成摘要'),
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
