import fs from "node:fs";

const meetingId = process.argv[2];
const marker = process.argv[3] ?? "【摘要人工校正】";
const outputPath = process.argv[4];
if (!meetingId) throw new Error("Usage: node cdp-t07-edit-save-copy.mjs <meeting-id> [marker]");

const cdpPort = process.env.CDP_PORT ?? "9233";
const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`).then((response) => response.json());
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
  const marker = ${JSON.stringify(marker)};
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  if (!location.href.includes(meetingId)) throw new Error('Not on target meeting');

  const summaryBefore = await invoke('api_get_summary', { meetingId });
  const historyBefore = await invoke('api_list_summary_generation_history', { meetingId });
  const hrefBefore = location.href;
  const editor = [...document.querySelectorAll('[contenteditable="true"]')]
    .find((element) => element.innerText.includes('Meetily') && element.innerText.length > 200);
  if (!editor) throw new Error('Visible summary editor was not found');
  let inserted = false;
  if (!editor.innerText.includes(marker)) {
    const target = editor.querySelector('p') ?? editor.querySelector('[data-content-type]') ?? editor;
    target.focus();
    const range = document.createRange();
    range.selectNodeContents(target);
    range.collapse(false);
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    inserted = document.execCommand('insertText', false, marker);
    if (!inserted) throw new Error('Browser rejected insertText command');
  }

  let saveButton = null;
  const dirtyStarted = performance.now();
  while (performance.now() - dirtyStarted < 5000) {
    saveButton = [...document.querySelectorAll('button')].find((button) =>
      (button.getAttribute('aria-label') ?? '').includes('保存')
      && !button.disabled
    );
    if (
      saveButton
      && editor.innerText.includes(marker)
      && String(saveButton.className).includes('bg-green-200')
    ) break;
    await delay(50);
  }
  if (!saveButton) throw new Error('Enabled save button was not found');
  const saveClassBefore = saveButton.className;
  if (!String(saveClassBefore).includes('bg-green-200')) {
    throw new Error('Summary editor did not reach dirty state before save');
  }
  saveButton.click();

  let summaryAfter = null;
  const saveStarted = performance.now();
  while (performance.now() - saveStarted < 10000) {
    summaryAfter = await invoke('api_get_summary', { meetingId });
    if (JSON.stringify(summaryAfter).includes(marker)) break;
    await delay(100);
  }
  if (!JSON.stringify(summaryAfter).includes(marker)) {
    throw new Error('Saved backend summary does not contain marker');
  }

  const copyButton = [...document.querySelectorAll('button')].find((button) =>
    (button.getAttribute('aria-label') ?? '').includes('复制摘要')
    && !button.disabled
  );
  if (!copyButton) throw new Error('Enabled copy summary button was not found');
  copyButton.click();
  await delay(800);
  let clipboardText = null;
  let clipboardError = null;
  try {
    clipboardText = await Promise.race([
      navigator.clipboard.readText(),
      delay(2000).then(() => { throw new Error('clipboard-read-timeout'); }),
    ]);
  } catch (error) {
    clipboardError = String(error);
  }

  const historyAfter = await invoke('api_list_summary_generation_history', { meetingId });
  return {
    capturedAt: new Date().toISOString(),
    meetingId,
    marker,
    hrefBefore,
    hrefAfter: location.href,
    inserted,
    saveClassBefore,
    editorContainsMarker: editor.innerText.includes(marker),
    summaryBefore,
    summaryAfter,
    historyBefore,
    historyAfter,
    clipboardText,
    clipboardError,
    visibleAlerts: [...document.querySelectorAll('[role="alert"]')]
      .filter((item) => item.getBoundingClientRect().width > 0)
      .map((item) => item.innerText),
    verdict: {
      backendSavedMarker: JSON.stringify(summaryAfter).includes(marker),
      editorContainsMarker: editor.innerText.includes(marker),
      copyContainsMarker: clipboardText?.includes(marker) ?? false,
      historyUnchangedByEditAndCopy: historyAfter.length === historyBefore.length,
      urlUnchanged: location.href === hrefBefore,
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
