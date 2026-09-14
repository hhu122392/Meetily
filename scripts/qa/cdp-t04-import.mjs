const expectedTitle = process.argv[2] ?? "QA-CORE-20260824-IMPORT";
const monitorMs = Math.max(1000, Number.parseInt(process.argv[3] ?? "45000", 10));
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
  const expectedTitle = ${JSON.stringify(expectedTitle)};
  const monitorMs = ${JSON.stringify(monitorMs)};
  const titleInput = [...document.querySelectorAll('input')].find(
    (input) => input.getAttribute('placeholder') === '输入会议标题'
  );
  if (!titleInput) throw new Error('Meeting title input was not found');

  const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
  setValue.call(titleInput, expectedTitle);
  titleInput.dispatchEvent(new Event('input', { bubbles: true }));
  titleInput.dispatchEvent(new Event('change', { bubbles: true }));
  await new Promise((resolve) => setTimeout(resolve, 100));

  const comboValues = [...document.querySelectorAll('button[role="combobox"]')]
    .map((button) => button.innerText.trim());
  if (!comboValues.includes('中文')) {
    throw new Error('Import language is not Chinese: ' + JSON.stringify(comboValues));
  }
  if (!comboValues.some((value) => value.includes('large-v3-turbo-q5_0'))) {
    throw new Error('Expected transcription model is not selected: ' + JSON.stringify(comboValues));
  }

  const importButton = [...document.querySelectorAll('button')].find(
    (button) => button.innerText.trim() === '导入' && !button.disabled
  );
  if (!importButton) throw new Error('Enabled Import button was not found');

  const startedAt = new Date().toISOString();
  const startedPerformance = performance.now();
  const observations = [];
  let priorKey = '';
  importButton.click();

  while (performance.now() - startedPerformance < monitorMs) {
    const text = document.body?.innerText ?? '';
    const href = location.href;
    const percent = (text.match(/(?:^|\\n)(\\d{1,3})%(?:$|\\n)/m) ?? [])[1] ?? null;
    const stage = ['正在复制音频', '正在解码音频', '正在重采样音频', '正在检测语音', '正在转写音频', '正在保存会议']
      .find((candidate) => text.includes(candidate)) ?? null;
    const error = text.includes('导入失败') || text.includes('处理失败');
    const complete = href.includes('/meeting-details?id=') && text.includes(expectedTitle);
    const key = JSON.stringify([href, percent, stage, error, complete]);
    if (key !== priorKey) {
      observations.push({
        at: new Date().toISOString(),
        elapsedMs: performance.now() - startedPerformance,
        href,
        percent,
        stage,
        error,
        complete,
        textTail: text.slice(-1000),
      });
      priorKey = key;
    }
    if (error || complete) break;
    await new Promise((resolve) => setTimeout(resolve, 250));
  }

  return {
    expectedTitle,
    selectedLanguage: comboValues[0],
    selectedModel: comboValues[1],
    titleValueBeforeClick: titleInput.value,
    startedAt,
    finishedAt: new Date().toISOString(),
    elapsedMs: performance.now() - startedPerformance,
    href: location.href,
    complete: location.href.includes('/meeting-details?id=') && (document.body?.innerText ?? '').includes(expectedTitle),
    errorVisible: (document.body?.innerText ?? '').includes('导入失败') || (document.body?.innerText ?? '').includes('处理失败'),
    observations,
    finalText: (document.body?.innerText ?? '').slice(-3000),
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
