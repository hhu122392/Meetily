import fs from "node:fs";
import path from "node:path";

const outputPath = process.argv[2];
const templateId = process.argv[3] ?? "license_station_weekly";
const expectedTemplateVersion = Number(process.argv[4] ?? "4");
if (!outputPath) {
  throw new Error(
    "Usage: node cdp-uat-environment-preflight.mjs <output.json> [template-id] [expected-version]",
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
  const templateId = ${JSON.stringify(templateId)};
  const expectedTemplateVersion = ${JSON.stringify(expectedTemplateVersion)};
  const invoke = window.__TAURI_INTERNALS__?.invoke;
  if (typeof invoke !== 'function') throw new Error('Tauri invoke bridge is unavailable');
  const capture = async (name, command, payload = {}) => {
    const started = performance.now();
    try {
      const value = await invoke(command, payload);
      return { name, ok: true, elapsedMs: performance.now() - started, value };
    } catch (error) {
      return { name, ok: false, elapsedMs: performance.now() - started, error: String(error) };
    }
  };
  const calls = await Promise.all([
    capture('recordingState', 'get_recording_state'),
    capture('modelConfig', 'api_get_model_config'),
    capture('models', 'builtin_ai_list_models'),
    capture('transcriptConfig', 'api_get_transcript_config'),
    capture('devices', 'get_audio_devices'),
    capture('preferences', 'get_recording_preferences'),
    capture('defaultRecordingsFolder', 'get_default_recordings_folder_path'),
    capture('template', 'api_get_template_v2', { request: { templateId } }),
    capture('templatesDirectory', 'api_get_templates_directory'),
  ]);
  const byName = Object.fromEntries(calls.map((item) => [item.name, item]));
  const modelConfig = byName.modelConfig.value ?? null;
  const models = byName.models.value ?? [];
  const selectedModel = models.find((item) => item.name === modelConfig?.model) ?? null;
  const transcriptConfig = byName.transcriptConfig.value ?? null;
  if (transcriptConfig && typeof transcriptConfig === 'object') {
    transcriptConfig.api_key = transcriptConfig.api_key ? '[REDACTED]' : null;
  }
  const devices = byName.devices.value ?? [];
  const inputDevices = devices.filter((item) => item.device_type === 'Input');
  const outputDevices = devices.filter((item) => item.device_type === 'Output');
  const template = byName.template.value?.template ?? null;
  const context = template?.extensions?.meetily_meeting_context ?? null;
  return {
    capturedAt: new Date().toISOString(),
    href: location.href,
    calls,
    modelConfig,
    selectedModel,
    transcriptConfig,
    devices,
    inputDevices,
    outputDevices,
    preferences: byName.preferences.value ?? null,
    defaultRecordingsFolder: byName.defaultRecordingsFolder.value ?? null,
    templatesDirectory: byName.templatesDirectory.value ?? null,
    template: byName.template.value ?? null,
    locale: {
      documentLanguage: document.documentElement.lang,
      uiLocale: localStorage.getItem('uiLocale'),
      uiLocalePreference: localStorage.getItem('uiLocalePreference'),
      primaryLanguage: localStorage.getItem('primaryLanguage'),
    },
    verdict: {
      tauriProductionOrigin: location.origin === 'http://tauri.localhost',
      requiredCallsSucceeded: calls.every((item) => item.ok),
      recordingIdle: byName.recordingState.value?.is_recording === false
        && byName.recordingState.value?.is_active === false,
      builtinTwoBSelected: modelConfig?.provider === 'builtin-ai' && modelConfig?.model === 'qwen3.5:2b',
      selectedModelAvailable: selectedModel?.status?.type === 'available',
      transcriptModelSelected: transcriptConfig?.model === 'large-v3-turbo-q5_0',
      hasInputDevice: inputDevices.length > 0,
      hasOutputDevice: outputDevices.length > 0,
      recordingsFolderConfigured: typeof byName.defaultRecordingsFolder.value === 'string'
        && byName.defaultRecordingsFolder.value.length > 0,
      expectedTemplateLoaded: template?.id === templateId
        && template?.version === expectedTemplateVersion,
      expectedPeopleLoaded: Array.isArray(context?.people) && context.people.filter((person) => person.enabled).length === 16,
      expectedTermsLoaded: Array.isArray(context?.terms) && context.terms.filter((term) => term.enabled).length === 8,
      simplifiedChineseUi: document.documentElement.lang.toLowerCase().startsWith('zh'),
      noPendingUiLock: document.querySelectorAll('[aria-busy="true"]').length === 0,
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
fs.mkdirSync(path.dirname(outputPath), { recursive: true });
fs.writeFileSync(outputPath, output, "utf8");
console.log(output.trimEnd());
socket.close();
