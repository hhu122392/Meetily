import fs from "node:fs";

const meetingId = process.argv[2];
const expectedTemplateId = process.argv[3] ?? "standard_meeting";
const outputPath = process.argv[4];
if (!meetingId) throw new Error("Usage: node cdp-t06-preflight.mjs <meeting-id> [expected-template-id] [output.json]");
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
  const expectedTemplateId = ${JSON.stringify(expectedTemplateId)};
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const capture = async (name, promise) => {
    try {
      return { name, ok: true, value: await promise };
    } catch (error) {
      return { name, ok: false, error: String(error) };
    }
  };
  const calls = await Promise.all([
    capture('modelConfig', invoke('api_get_model_config')),
    capture('models', invoke('builtin_ai_list_models')),
    capture('autoGenerate', invoke('api_get_auto_generate_setting')),
    capture('preference', invoke('api_get_meeting_template_preference', { request: { meetingId } })),
    capture('history', invoke('api_list_summary_generation_history', { meetingId })),
    capture('meeting', invoke('api_get_meeting', { meetingId })),
  ]);
  const byName = Object.fromEntries(calls.map((item) => [item.name, item]));
  const modelConfig = byName.modelConfig.value ?? null;
  const models = byName.models.value ?? [];
  const autoGenerate = byName.autoGenerate.value ?? null;
  const preference = byName.preference.value ?? null;
  const history = byName.history.value ?? [];
  const meeting = byName.meeting.value ?? null;
  const selectedModel = models.find((item) => item.name === modelConfig.model) ?? null;
  return {
    capturedAt: new Date().toISOString(),
    href: location.href,
    calls,
    modelConfig,
    selectedModel,
    models,
    autoGenerate,
    preference,
    history,
    meeting,
    localStorage: {
      summaryLanguage: localStorage.getItem('summaryLanguage'),
      summaryLanguagePreferences: localStorage.getItem('summary-language-preferences'),
      primaryLanguage: localStorage.getItem('primaryLanguage'),
      uiLocale: localStorage.getItem('uiLocale'),
    },
    visible: {
      title: document.querySelector('h1')?.textContent?.trim() ?? null,
      templateButton: [...document.querySelectorAll('button')].find(
        (button) => button.getAttribute('aria-label') === '选择会议总结模板'
      )?.textContent?.trim() ?? null,
      languageButton: [...document.querySelectorAll('button')].find(
        (button) => button.getAttribute('aria-label') === '设置摘要语言'
      )?.textContent?.trim() ?? null,
      generateButtons: [...document.querySelectorAll('button')]
        .filter((button) => /生成.*摘要/.test(button.innerText.trim()) || /生成.*摘要/.test(button.title))
        .map((button) => ({ text: button.innerText.trim(), title: button.title, disabled: button.disabled })),
    },
    verdict: {
      correctMeeting: location.href.includes(meetingId),
      requiredPreflightCallsSucceeded: calls
        .filter((item) => item.name !== 'autoGenerate')
        .every((item) => item.ok),
      optionalAutoGenerateCommandAvailable: byName.autoGenerate.ok,
      providerIsBuiltin: modelConfig?.provider === 'builtin-ai',
      selectedModelMatches: selectedModel?.name === modelConfig?.model,
      selectedModelAvailable: selectedModel?.status?.type === 'available',
      noApiKeyRequired: modelConfig?.provider === 'builtin-ai',
      templateBound: preference?.preference?.mode === 'meeting_override'
        && preference?.resolved?.templateId === expectedTemplateId
        && preference?.resolved?.source === 'meeting_override',
      noPendingUiLock: [...document.querySelectorAll('[aria-busy="true"]')].length === 0,
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
