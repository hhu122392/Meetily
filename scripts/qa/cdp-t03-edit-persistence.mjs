import fs from "node:fs";

const meetingId = process.argv[2];
const screenshotPath = process.argv[3];
const expectedTitleArg = process.argv[4] ?? "QA-CORE-20260824-LIVE";
const outputPath = process.argv[5];
if (!meetingId) {
  console.error("Usage: node cdp-t03-edit-persistence.mjs <meeting-id> [screenshot.png]");
  process.exit(2);
}

const cdpPort = process.env.CDP_PORT ?? "9222";
const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`).then((response) =>
  response.json(),
);
const page = targets.find(
  (target) => target.type === "page" && target.url.startsWith("http://tauri.localhost"),
);
if (!page) throw new Error("Meetily WebView2 release target was not found");

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
async function evaluate(expression) {
  const evaluated = await call("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
    userGesture: true,
  });
  if (evaluated.exceptionDetails) {
    throw new Error(evaluated.exceptionDetails.text ?? "Runtime.evaluate failed");
  }
  return evaluated.result.value;
}

await call("Runtime.enable");
await call("Page.enable");
await call("Emulation.setDeviceMetricsOverride", {
  width: 1600,
  height: 900,
  deviceScaleFactor: 1,
  mobile: false,
});
const destination = `http://tauri.localhost/meeting-details?id=${encodeURIComponent(meetingId)}&source=recording`;
await call("Page.navigate", { url: destination });
await new Promise((resolve) => setTimeout(resolve, 2500));

const result = await evaluate(`(async () => {
  const meetingId = ${JSON.stringify(meetingId)};
  const savedMarker = '【本轮人工校正】';
  const cancelledMarker = '【本轮不应保存】';
  const expectedTitle = ${JSON.stringify(expectedTitleArg)};
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const waitFor = async (predicate, label, timeoutMs = 10000) => {
    const started = performance.now();
    while (performance.now() - started < timeoutMs) {
      const value = await predicate();
      if (value) return value;
      await sleep(50);
    }
    throw new Error('Timed out waiting for ' + label);
  };
  const setNativeValue = (element, value) => {
    const prototype = element instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(prototype, 'value')?.set;
    if (!setter) throw new Error('Native value setter unavailable');
    setter.call(element, value);
    element.dispatchEvent(new Event('input', { bubbles: true }));
    element.dispatchEvent(new Event('change', { bubbles: true }));
  };
  const clickWithoutBubble = (button) => new Promise((resolve) => {
    let stopCalls = 0;
    const originalStopPropagation = Event.prototype.stopPropagation;
    Event.prototype.stopPropagation = function(...args) {
      if (this.target === button || button.contains(this.target)) stopCalls += 1;
      return originalStopPropagation.apply(this, args);
    };
    button.click();
    setTimeout(() => {
      Event.prototype.stopPropagation = originalStopPropagation;
      resolve(stopCalls > 0 ? 0 : 1);
    }, 0);
  });
  const commandCalls = [];
  const originalInvoke = window.__TAURI_INTERNALS__.invoke;
  window.__TAURI_INTERNALS__.invoke = function(command, payload, ...rest) {
    commandCalls.push({ command, payload, at: new Date().toISOString() });
    return originalInvoke.call(this, command, payload, ...rest);
  };

  try {
    const backendBefore = await originalInvoke('api_get_meeting', { meetingId });
    await waitFor(
      () => document.querySelectorAll('[data-transcript-id]').length >= 2,
      'transcript segments',
    );
    const before = {
      capturedAt: new Date().toISOString(),
      href: location.href,
      titleText: document.querySelector('h1')?.textContent?.trim() ?? null,
      segmentCount: document.querySelectorAll('[data-transcript-id]').length,
      editButtonCount: [...document.querySelectorAll('button')].filter(
        (button) => button.getAttribute('aria-label') === '编辑转写片段',
      ).length,
      bodyHasSavedMarker: document.body.innerText.includes(savedMarker),
      bodyHasCancelledMarker: document.body.innerText.includes(cancelledMarker),
    };

    const titleEdit = [...document.querySelectorAll('button')].find(
      (button) => button.getAttribute('aria-label') === '编辑会议标题',
    );
    if (!titleEdit) throw new Error('Title edit button not found');
    titleEdit.click();
    const titleInput = await waitFor(
      () => [...document.querySelectorAll('textarea')].find(
        (textarea) => textarea.className.includes('text-2xl'),
      ),
      'title editor',
    );
    titleInput.focus();
    await sleep(50);
    setNativeValue(titleInput, expectedTitle);
    // A real user cannot type and blur in the same JavaScript task. Allow
    // React to commit the dirty state before exercising the blur-save path.
    await sleep(100);
    titleInput.dispatchEvent(new FocusEvent('focusout', {
      bubbles: true,
      relatedTarget: document.body,
    }));
    titleInput.blur();
    const savedMeeting = await waitFor(async () => {
      const current = await originalInvoke('api_get_meeting', { meetingId });
      return current?.title === expectedTitle ? current : null;
    }, 'title to persist after editing finishes', 15000);

    const firstSegment = document.querySelector('[data-transcript-id]');
    const firstId = firstSegment.dataset.transcriptId;
    const firstOriginal = firstSegment.innerText;
    const firstEdit = firstSegment.querySelector('button[aria-label="编辑转写片段"]');
    const firstEditBubbled = await clickWithoutBubble(firstEdit);
    const firstTextarea = await waitFor(
      () => document.querySelector('[data-transcript-id="' + CSS.escape(firstId) + '"] textarea[aria-label="编辑转写片段文本"]'),
      'first transcript editor',
    );
    setNativeValue(firstTextarea, firstTextarea.value + savedMarker);
    const firstSave = firstSegment.querySelector('button[aria-label="保存校正"]');
    const firstSaveBubbled = await clickWithoutBubble(firstSave);
    await waitFor(
      () => {
        const segment = document.querySelector('[data-transcript-id="' + CSS.escape(firstId) + '"]');
        return segment && !segment.querySelector('textarea') && segment.innerText.includes(savedMarker);
      },
      'saved transcript marker',
      15000,
    );

    const allSegments = [...document.querySelectorAll('[data-transcript-id]')];
    const secondSegment = allSegments.find((segment) => segment.dataset.transcriptId !== firstId);
    const secondId = secondSegment.dataset.transcriptId;
    const secondOriginal = secondSegment.innerText;
    const secondEdit = secondSegment.querySelector('button[aria-label="编辑转写片段"]');
    const secondEditBubbled = await clickWithoutBubble(secondEdit);
    const secondTextarea = await waitFor(
      () => document.querySelector('[data-transcript-id="' + CSS.escape(secondId) + '"] textarea[aria-label="编辑转写片段文本"]'),
      'second transcript editor',
    );
    setNativeValue(secondTextarea, secondTextarea.value + cancelledMarker);
    const secondCancel = secondSegment.querySelector('button[aria-label="取消转写编辑"]');
    const secondCancelBubbled = await clickWithoutBubble(secondCancel);
    await waitFor(
      () => {
        const segment = document.querySelector('[data-transcript-id="' + CSS.escape(secondId) + '"]');
        return segment && !segment.querySelector('textarea');
      },
      'cancelled transcript editor to close',
    );

    await sleep(500);
    const backendAfter = await originalInvoke('api_get_meeting', { meetingId });
    const beforeTextById = new Map(backendBefore.transcripts.map((item) => [item.id, item.text]));
    const changedTranscriptIds = backendAfter.transcripts
      .filter((item) => beforeTextById.get(item.id) !== item.text)
      .map((item) => item.id);
    const afterFirst = document.querySelector('[data-transcript-id="' + CSS.escape(firstId) + '"]');
    const afterSecond = document.querySelector('[data-transcript-id="' + CSS.escape(secondId) + '"]');
    return {
      before,
      after: {
        capturedAt: new Date().toISOString(),
        href: location.href,
        titleText: document.querySelector('h1')?.textContent?.trim() ?? null,
        persistedTitle: savedMeeting.title,
        firstId,
        firstOriginal,
        firstText: afterFirst?.innerText ?? null,
        firstHasSavedMarker: afterFirst?.innerText.includes(savedMarker) ?? false,
        secondId,
        secondOriginal,
        secondText: afterSecond?.innerText ?? null,
        secondHasCancelledMarker: afterSecond?.innerText.includes(cancelledMarker) ?? false,
        bodyHasCancelledMarker: document.body.innerText.includes(cancelledMarker),
      },
      bubbling: {
        firstEditBubbled,
        firstSaveBubbled,
        secondEditBubbled,
        secondCancelBubbled,
      },
      persistence: {
        backendBeforeTitle: backendBefore.title,
        backendAfterTitle: backendAfter.title,
        changedTranscriptIds,
        firstPersistedText: backendAfter.transcripts.find((item) => item.id === firstId)?.text ?? null,
        secondPersistedText: backendAfter.transcripts.find((item) => item.id === secondId)?.text ?? null,
      },
      commandCalls,
      commandCounts: Object.fromEntries(
        [...new Set(commandCalls.map((call) => call.command))].map((command) => [
          command,
          commandCalls.filter((call) => call.command === command).length,
        ]),
      ),
      verdict: {
        titleChanged: document.querySelector('h1')?.textContent?.trim() === expectedTitle
          && savedMeeting.title === expectedTitle,
        savedMarkerVisible: afterFirst?.innerText.includes(savedMarker) ?? false,
        cancelledMarkerAbsent: !(afterSecond?.innerText.includes(cancelledMarker) ?? true),
        editControlsDidNotBubble: [firstEditBubbled, firstSaveBubbled, secondEditBubbled, secondCancelBubbled].every((count) => count === 0),
        exactlyOneTranscriptUpdate: changedTranscriptIds.length === 1
          && changedTranscriptIds[0] === firstId
          && (backendAfter.transcripts.find((item) => item.id === firstId)?.text ?? '').includes(savedMarker)
          && !(backendAfter.transcripts.find((item) => item.id === secondId)?.text ?? '').includes(cancelledMarker),
        noParentAction: !commandCalls.some((call) => [
          'api_process_transcript',
          'start_recording',
          'start_recording_with_devices',
          'start_retranscription',
        ].includes(call.command)),
      },
    };
  } finally {
    window.__TAURI_INTERNALS__.invoke = originalInvoke;
  }
})()`);

if (screenshotPath) {
  const screenshot = await call("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(screenshotPath, Buffer.from(screenshot.data, "base64"));
}
const output = `${JSON.stringify(result, null, 2)}\n`;
if (outputPath) fs.writeFileSync(outputPath, output, "utf8");
console.log(output.trimEnd());
socket.close();
