const meetingId = process.argv[2];
const marker = process.argv[3] ?? "【未保存摘要】";
if (!meetingId) throw new Error("Usage: node cdp-t07-unsaved-guard.mjs <meeting-id> [marker]");
const cdpPort = process.env.CDP_PORT ?? "9233";
const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`).then((response) => response.json());
const page = targets.find((target) => target.type === "page" && target.url.startsWith("http://tauri.localhost"));
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
  const historyBefore = await invoke('api_list_summary_generation_history', { meetingId });
  const savedBefore = await invoke('api_get_summary', { meetingId });
  const editor = [...document.querySelectorAll('[contenteditable="true"]')]
    .find((element) => element.getBoundingClientRect().width > 300 && element.innerText.includes('M100'));
  if (!editor) throw new Error('Visible summary editor was not found');
  if (!editor.innerText.includes(marker)) {
    const target = editor.querySelector('p') ?? editor;
    target.focus();
    const range = document.createRange();
    range.selectNodeContents(target);
    range.collapse(false);
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    if (!document.execCommand('insertText', false, marker)) throw new Error('insertText failed');
  }
  let saveButton = null;
  const dirtyStarted = performance.now();
  while (performance.now() - dirtyStarted < 5000) {
    saveButton = [...document.querySelectorAll('button')].find((button) =>
      (button.getAttribute('aria-label') ?? '').includes('保存更改')
    );
    if (saveButton && String(saveButton.className).includes('bg-green-200')) break;
    await delay(50);
  }
  const regenerateButton = [...document.querySelectorAll('button')].find((button) =>
    button.title === '重新生成 AI 摘要' && !button.disabled
  );
  if (!regenerateButton) throw new Error('Regenerate button was not found');
  regenerateButton.click();
  await delay(300);
  const dialogs = [...document.querySelectorAll('[role="dialog"]')]
    .filter((dialog) => dialog.getBoundingClientRect().width > 0)
    .map((dialog) => dialog.innerText);
  const bodyText = document.body?.innerText ?? '';
  const hasExplicitUnsavedPrompt = dialogs.some((text) =>
    /未保存|保存后继续|放弃修改|unsaved|save and continue/i.test(text)
  );
  return {
    capturedAt: new Date().toISOString(),
    meetingId,
    marker,
    dirtyBeforeRegenerate: Boolean(saveButton && String(saveButton.className).includes('bg-green-200')),
    dialogs,
    hasExplicitUnsavedPrompt,
    editorStillContainsMarker: editor.innerText.includes(marker),
    backendStillExcludesMarker: !JSON.stringify(await invoke('api_get_summary', { meetingId })).includes(marker),
    historyBefore,
    historyAfter: await invoke('api_list_summary_generation_history', { meetingId }),
    bodyTail: bodyText.slice(-3000),
    savedBefore,
    verdict: hasExplicitUnsavedPrompt ? 'PASS' : 'FAIL',
  };
})()`;
const evaluated = await call("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true, userGesture: true });
if (evaluated.exceptionDetails) throw new Error(evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text);
console.log(JSON.stringify(evaluated.result.value, null, 2));
await call("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
await call("Input.dispatchKeyEvent", { type: "keyUp", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
socket.close();
