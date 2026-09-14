import fs from "node:fs";

const meetingId = process.argv[2];
const monitorMs = Math.max(1000, Number.parseInt(process.argv[3] ?? "60000", 10));
const outputPath = process.argv[4];
if (!meetingId) throw new Error("Usage: node cdp-t04-retranscribe.mjs <meeting-id> [monitor-ms] [output.json]");

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
  const monitorMs = ${JSON.stringify(monitorMs)};
  let comboValues = [...document.querySelectorAll('button[role="combobox"]')]
    .map((button) => button.innerText.trim());
  if (!comboValues.includes('中文')) {
    const enhanceButton = [...document.querySelectorAll('button')].find(
      (button) => button.innerText.trim() === '增强' && !button.disabled
    );
    if (!enhanceButton) throw new Error('Enabled enhance button was not found');
    enhanceButton.click();
    const dialogDeadline = performance.now() + 30000;
    while (performance.now() < dialogDeadline) {
      comboValues = [...document.querySelectorAll('button[role="combobox"]')]
        .map((button) => button.innerText.trim());
      if (comboValues.includes('中文')) break;
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
  }
  const modelsDeadline = performance.now() + 30000;
  while (comboValues.length < 2 && performance.now() < modelsDeadline) {
    await new Promise((resolve) => setTimeout(resolve, 100));
    comboValues = [...document.querySelectorAll('button[role="combobox"]')]
      .map((button) => button.innerText.trim());
  }
  if (!comboValues.includes('中文')) {
    throw new Error('Retranscription language is not Chinese: ' + JSON.stringify(comboValues));
  }
  if (!comboValues.some((value) => value.includes('large-v3-turbo-q5_0'))) {
    throw new Error('Expected transcription model is not selected: ' + JSON.stringify(comboValues));
  }
  const startButton = [...document.querySelectorAll('button')].find(
    (button) => button.innerText.trim() === '开始重新转写' && !button.disabled
  );
  if (!startButton) throw new Error('Enabled retranscription button was not found');

  const startedAt = new Date().toISOString();
  const startedPerformance = performance.now();
  const observations = [];
  let priorKey = '';
  let seenBusy = false;
  startButton.click();

  while (performance.now() - startedPerformance < monitorMs) {
    const text = document.body?.innerText ?? '';
    const href = location.href;
    const percent = (text.match(/(?:^|\\n)(\\d{1,3})%(?:$|\\n)/m) ?? [])[1] ?? null;
    const stage = ['正在解码音频', '正在检测语音', '正在转写音频', '正在保存转写', '正在重新转写']
      .find((candidate) => text.includes(candidate)) ?? null;
    const error = text.includes('重新转写失败');
    const dialogOpen = text.includes('重新转写会议') || text.includes('正在重新转写');
    if (dialogOpen || stage) seenBusy = true;
    const complete = seenBusy && !dialogOpen && href.includes(meetingId);
    const key = JSON.stringify([href, percent, stage, error, dialogOpen, complete]);
    if (key !== priorKey) {
      observations.push({
        at: new Date().toISOString(),
        elapsedMs: performance.now() - startedPerformance,
        href,
        percent,
        stage,
        error,
        dialogOpen,
        complete,
        textTail: text.slice(-1200),
      });
      priorKey = key;
    }
    if (error || complete) break;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  const finalText = document.body?.innerText ?? '';
  const meeting = await window.__TAURI_INTERNALS__.invoke('api_get_meeting', { meetingId });
  return {
    meetingId,
    selectedLanguage: comboValues[0],
    selectedModel: comboValues[1],
    startedAt,
    finishedAt: new Date().toISOString(),
    elapsedMs: performance.now() - startedPerformance,
    href: location.href,
    complete: !finalText.includes('重新转写会议') && !finalText.includes('正在重新转写') && location.href.includes(meetingId),
    errorVisible: finalText.includes('重新转写失败'),
    observations,
    meeting,
    finalText: finalText.slice(-3000),
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
