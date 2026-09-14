import fs from "node:fs";

const meetingId = process.argv[2];
const otherMeetingId = process.argv[3];
const templateId = process.argv[4] ?? "standard_meeting";
const templateName = process.argv[5] ?? "标准会议纪要";
const action = process.argv[6] ?? "audit";
const outputPath = process.argv[7];
if (!meetingId || !otherMeetingId) {
  throw new Error(
    "Usage: node cdp-t05-template-binding.mjs <meeting-id> <other-meeting-id> [template-id] [template-name] [audit|bind]",
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
  const otherMeetingId = ${JSON.stringify(otherMeetingId)};
  const templateId = ${JSON.stringify(templateId)};
  const templateName = ${JSON.stringify(templateName)};
  const action = ${JSON.stringify(action)};
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const getPreference = (id) => invoke('api_get_meeting_template_preference', {
    request: { meetingId: id },
  });
  const getHistory = (id) => invoke('api_list_summary_generation_history', { meetingId: id });
  const before = {
    capturedAt: new Date().toISOString(),
    href: location.href,
    preference: await getPreference(meetingId),
    otherPreference: await getPreference(otherMeetingId),
    history: await getHistory(meetingId),
    otherHistory: await getHistory(otherMeetingId),
    selectedButtonText: [...document.querySelectorAll('button')].find(
      (button) => button.getAttribute('aria-label') === '选择会议总结模板'
    )?.innerText.trim() ?? null,
  };

  if (action === 'bind') {
    if (!location.href.includes(meetingId)) {
      throw new Error('Current page is not the target meeting: ' + location.href);
    }
    const selector = [...document.querySelectorAll('button')].find(
      (button) => button.getAttribute('aria-label') === '选择会议总结模板' && !button.disabled
    );
    if (!selector) throw new Error('Template selector was not found or disabled');
    selector.click();
    const openStarted = performance.now();
    while (performance.now() - openStarted < 5000) {
      if ((document.body?.innerText ?? '').includes('会议总结模板')) break;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    const templateButton = [...document.querySelectorAll('button')].find((button) =>
      [...button.querySelectorAll('p')].some((paragraph) => paragraph.textContent?.trim() === templateName)
    );
    if (!templateButton) throw new Error('Template option was not found: ' + templateName);
    const clickedAt = new Date().toISOString();
    templateButton.click();
    const saveStarted = performance.now();
    let preference = null;
    let lastError = null;
    while (performance.now() - saveStarted < 10000) {
      try {
        preference = await getPreference(meetingId);
        if (preference.preference.mode === 'meeting_override' && preference.preference.templateId === templateId) {
          break;
        }
      } catch (error) {
        lastError = String(error);
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    if (!preference || preference.preference.mode !== 'meeting_override' || preference.preference.templateId !== templateId) {
      throw new Error('Template binding did not persist: ' + JSON.stringify({ preference, lastError }));
    }
    await new Promise((resolve) => setTimeout(resolve, 300));
    before.bindingAction = {
      clickedAt,
      elapsedMs: performance.now() - saveStarted,
      selectorBusyAfterSave: selector.getAttribute('aria-busy'),
      visibleAlerts: [...document.querySelectorAll('[role="alert"]')]
        .filter((item) => {
          const rect = item.getBoundingClientRect();
          return rect.width > 0 && rect.height > 0;
        })
        .map((item) => item.innerText),
      bodyTail: (document.body?.innerText ?? '').slice(-1800),
    };
  }

  const after = {
    capturedAt: new Date().toISOString(),
    href: location.href,
    preference: await getPreference(meetingId),
    otherPreference: await getPreference(otherMeetingId),
    history: await getHistory(meetingId),
    otherHistory: await getHistory(otherMeetingId),
    selectedButtonText: [...document.querySelectorAll('button')].find(
      (button) => button.getAttribute('aria-label') === '选择会议总结模板'
    )?.innerText.trim() ?? null,
  };

  return {
    action,
    meetingId,
    otherMeetingId,
    templateId,
    templateName,
    before,
    after,
    verdict: {
      targetBound: after.preference.preference.mode === 'meeting_override'
        && after.preference.preference.templateId === templateId
        && after.preference.resolved.templateId === templateId
        && after.preference.resolved.source === 'meeting_override',
      metadataStorage: after.preference.storage === 'metadata',
      otherMeetingNotOverridden: after.otherPreference.preference.mode !== 'meeting_override'
        || after.otherPreference.preference.templateId !== templateId,
      urlUnchanged: before.href === after.href,
      targetHistoryUnchanged: before.history.length === after.history.length,
      otherHistoryUnchanged: before.otherHistory.length === after.otherHistory.length,
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
