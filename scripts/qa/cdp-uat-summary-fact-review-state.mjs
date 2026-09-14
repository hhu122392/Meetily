import fs from "node:fs";

const meetingId = process.argv[2];
const outputPath = process.argv[3];
if (!meetingId || !outputPath) {
  throw new Error(
    "Usage: node cdp-uat-summary-fact-review-state.mjs <meeting-id> <output.json>",
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
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const [summary, history, recording, retranscription] = await Promise.all([
    invoke('api_get_summary', { meetingId }),
    invoke('api_list_summary_generation_history', { meetingId }),
    invoke('get_recording_state'),
    invoke('is_retranscription_in_progress_command'),
  ]);
  const visibleAlerts = [...document.querySelectorAll('[role="alert"]')]
    .filter((element) => {
      const rect = element.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    })
    .map((element) => element.innerText.trim());
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
  const factValidation = summary?.data?.factValidation ?? null;
  const warningCodes = factValidation?.warnings?.map((warning) => warning.code) ?? [];
  const expectedWarnings = [
    'unsupported_transcript_term',
    'unsupported_organization',
    'unsupported_status_claim',
  ];
  const newest = history[0] ?? null;
  return {
    capturedAt: new Date().toISOString(),
    href: location.href,
    meetingId,
    processOrigin: location.origin,
    summaryStatus: summary?.status ?? null,
    factValidation,
    templateSnapshot: summary?.data?.template_snapshot ?? null,
    markdown: summary?.data?.markdown ?? null,
    history,
    newest,
    recording,
    retranscription,
    visibleAlerts,
    factReviewCards,
    errorCards,
    visibleTextTail: (document.body?.innerText ?? '').slice(-5000),
    verdict: {
      correctMeeting: location.href.includes(meetingId),
      productionOrigin: location.origin === 'http://tauri.localhost',
      generationCompleted: summary?.status === 'completed' && newest?.status === 'completed',
      needsReviewPersisted: factValidation?.status === 'needs_review',
      expectedWarningsPresent: expectedWarnings.every((code) => warningCodes.includes(code)),
      exactlyThreeGroundingWarnings: factValidation?.warningCount === 3,
      factReviewVisible: factReviewCards.length === 1,
      noVisibleGenerationError: errorCards.length === 0,
      builtinTwoB: newest?.modelProvider === 'builtin-ai' && newest?.modelName === 'qwen3.5:2b',
      expectedTemplate: newest?.templateId === 'license_station_weekly' && newest?.templateVersion === 4,
      recordingIdle: recording?.is_recording === false && recording?.is_active === false,
      retranscriptionIdle: retranscription === false,
    },
  };
})()`;
const evaluated = await call("Runtime.evaluate", {
  expression,
  returnByValue: true,
  awaitPromise: true,
});
if (evaluated.exceptionDetails) {
  throw new Error(evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text);
}
const output = `${JSON.stringify(evaluated.result.value, null, 2)}\n`;
fs.writeFileSync(outputPath, output, "utf8");
console.log(output.trimEnd());
socket.close();
