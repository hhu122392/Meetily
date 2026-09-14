import fs from "node:fs";

const meetingId = process.argv[2];
const outputPath = process.argv[3];
const screenshotPath = process.argv[4];
if (!meetingId || !outputPath) {
  throw new Error(
    "Usage: node cdp-uat06-edit-persistence.mjs <meeting-id> <output.json> [screenshot.png]",
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
await call("Page.enable");
await call("Emulation.setDeviceMetricsOverride", {
  width: 1600,
  height: 1000,
  deviceScaleFactor: 1,
  mobile: false,
});

const expression = `(async () => {
  const meetingId = ${JSON.stringify(meetingId)};
  const correctedSummarySentence = '本周仍计划签约 8 个 YouTube 新博主；后续量化目标待下周根据实际数据表现确定。';
  const invokeBridge = window.__TAURI_INTERNALS__;
  const originalInvoke = invokeBridge.invoke;
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const waitFor = async (predicate, label, timeoutMs = 15000) => {
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
      resolve({ stopCalls, bubbled: stopCalls === 0 });
    }, 0);
  });
  const readState = async () => {
    const [meeting, summary, history, manualRevisions, recording, retranscription] =
      await Promise.all([
        originalInvoke('api_get_meeting', { meetingId }),
        originalInvoke('api_get_summary', { meetingId }),
        originalInvoke('api_list_summary_generation_history', { meetingId }),
        originalInvoke('api_list_manual_summary_revisions', { meetingId }),
        originalInvoke('get_recording_state'),
        originalInvoke('is_retranscription_in_progress_command'),
      ]);
    return { meeting, summary, history, manualRevisions, recording, retranscription };
  };

  if (!location.href.includes(meetingId)) throw new Error('Not on target meeting');
  const before = await readState();
  const sourceGenerationId = before.history[0]?.generationId ?? null;
  const transcriptTarget = before.meeting.transcripts.find((segment) =>
    segment.text.includes('博主也是发同样的贴子')
  );
  if (!transcriptTarget) throw new Error('Real transcript typo target was not found');
  if (!before.summary?.data?.markdown?.includes('YouTube币目标设定')) {
    throw new Error('Real summary correction target was not found');
  }

  const commandCalls = [];
  invokeBridge.invoke = function(command, payload, ...rest) {
    commandCalls.push({ command, payload, at: new Date().toISOString() });
    return originalInvoke.call(this, command, payload, ...rest);
  };

  try {
    const transcriptElement = await waitFor(
      () => document.querySelector('[data-transcript-id="' + CSS.escape(transcriptTarget.id) + '"]'),
      'real transcript target element',
    );
    transcriptElement.scrollIntoView({ block: 'center' });
    const editButton = transcriptElement.querySelector('button[aria-label="编辑转写片段"]');
    if (!editButton) throw new Error('Transcript edit button not found');
    const editBubble = await clickWithoutBubble(editButton);
    const textarea = await waitFor(
      () => document.querySelector(
        '[data-transcript-id="' + CSS.escape(transcriptTarget.id) + '"] textarea[aria-label="编辑转写片段文本"]'
      ),
      'transcript textarea',
    );
    const temporaryCorrection = textarea.value.replace('贴子', '帖子');
    if (temporaryCorrection === textarea.value) throw new Error('Temporary typo correction was not applied');
    setNativeValue(textarea, temporaryCorrection);
    const cancelButton = transcriptElement.querySelector('button[aria-label="取消转写编辑"]');
    if (!cancelButton) throw new Error('Transcript cancel button not found');
    const cancelBubble = await clickWithoutBubble(cancelButton);
    await waitFor(
      () => !document.querySelector(
        '[data-transcript-id="' + CSS.escape(transcriptTarget.id) + '"] textarea'
      ),
      'transcript editor to close after cancel',
    );
    const meetingAfterCancel = await originalInvoke('api_get_meeting', { meetingId });
    const transcriptAfterCancel = meetingAfterCancel.transcripts.find(
      (segment) => segment.id === transcriptTarget.id,
    );

    const editor = await waitFor(
      () => [...document.querySelectorAll('[contenteditable="true"]')].find(
        (element) => element.innerText.includes('YouTube币目标设定') && element.innerText.length > 500,
      ),
      'summary editor',
    );
    const targetInline = [...editor.querySelectorAll('.bn-inline-content')].find(
      (element) => element.innerText.includes('YouTube币目标设定'),
    );
    if (!targetInline) throw new Error('Summary target block not found');
    targetInline.scrollIntoView({ block: 'center' });
    targetInline.focus();
    const range = document.createRange();
    range.selectNodeContents(targetInline);
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    const inserted = document.execCommand('insertText', false, correctedSummarySentence);
    if (!inserted) throw new Error('Browser rejected summary correction');

    const saveButton = await waitFor(
      () => [...document.querySelectorAll('button')].find((button) =>
        button.getAttribute('aria-label') === '保存更改'
        && !button.disabled
        && String(button.className).includes('bg-green-200')
      ),
      'dirty summary save button',
    );
    const saveClassBefore = saveButton.className;
    let saveButtonClicks = 0;
    saveButton.click();
    saveButtonClicks += 1;

    const after = await waitFor(async () => {
      const current = await readState();
      const saved = current.summary?.data?.markdown?.includes(correctedSummarySentence)
        || JSON.stringify(current.summary?.data?.summary_json ?? []).includes(correctedSummarySentence);
      const revisionAdded = current.manualRevisions.length === before.manualRevisions.length + 1;
      return saved && revisionAdded ? current : null;
    }, 'summary correction and manual revision to persist', 20000);

    const commandCounts = Object.fromEntries(
      [...new Set(commandCalls.map((call) => call.command))].map((command) => [
        command,
        commandCalls.filter((call) => call.command === command).length,
      ]),
    );
    const latestRevision = after.manualRevisions[0] ?? null;
    const beforeFactValidation = before.summary?.data?.factValidation ?? null;
    const afterFactValidation = after.summary?.data?.factValidation ?? null;
    const beforeSnapshot = before.summary?.data?.template_snapshot ?? null;
    const afterSnapshot = after.summary?.data?.template_snapshot ?? null;
    const forbiddenCommands = commandCalls.filter((call) => [
      'api_process_transcript',
      'start_recording',
      'start_recording_with_devices',
      'start_retranscription',
      'start_retranscription_command',
      'api_update_transcript_segment',
    ].includes(call.command));

    return {
      capturedAt: new Date().toISOString(),
      meetingId,
      correctedSummarySentence,
      transcriptCancel: {
        transcriptId: transcriptTarget.id,
        originalText: transcriptTarget.text,
        temporaryCorrection,
        backendTextAfterCancel: transcriptAfterCancel?.text ?? null,
        editBubble,
        cancelBubble,
      },
      summaryEdit: {
        inserted,
        saveClassBefore,
        saveButtonClicks,
        sourceGenerationId,
        latestRevision,
      },
      before,
      after,
      commandCalls,
      commandCounts,
      forbiddenCommands,
      verdict: {
        transcriptCancelRestoredOriginal: transcriptAfterCancel?.text === transcriptTarget.text,
        transcriptControlsStoppedPropagation: !editBubble.bubbled && !cancelBubble.bubbled,
        noTranscriptUpdateCommand: (commandCounts.api_update_transcript_segment ?? 0) === 0,
        exactlyOneSummarySave: saveButtonClicks === 1
          && after.manualRevisions.length === before.manualRevisions.length + 1,
        invokeCommandTraceAvailable: commandCalls.length > 0,
        correctedSummaryPersisted: after.summary?.data?.markdown?.includes(correctedSummarySentence)
          || JSON.stringify(after.summary?.data?.summary_json ?? []).includes(correctedSummarySentence),
        exactlyOneManualRevisionAdded: after.manualRevisions.length === before.manualRevisions.length + 1,
        generationHistoryUnchanged: JSON.stringify(after.history) === JSON.stringify(before.history),
        factValidationPreserved: JSON.stringify(afterFactValidation) === JSON.stringify(beforeFactValidation),
        templateSnapshotPreserved: JSON.stringify(afterSnapshot) === JSON.stringify(beforeSnapshot),
        sourceGenerationPreserved: latestRevision?.sourceGenerationId === sourceGenerationId,
        noForbiddenBackgroundCommand: forbiddenCommands.length === 0,
        recordingIdle: after.recording?.is_recording === false && after.recording?.is_active === false,
        retranscriptionIdle: after.retranscription === false,
        urlUnchanged: location.href.includes(meetingId),
      },
    };
  } finally {
    invokeBridge.invoke = originalInvoke;
  }
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
fs.writeFileSync(outputPath, output, "utf8");
if (screenshotPath) {
  const screenshot = await call("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(screenshotPath, Buffer.from(screenshot.data, "base64"));
}
console.log(output.trimEnd());
socket.close();
