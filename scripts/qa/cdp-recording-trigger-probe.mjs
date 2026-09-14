const action = process.argv[2] ?? 'read';
const cdpPort = process.env.CDP_PORT ?? '9233';
const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`).then((response) => response.json());
const page = targets.find(
  (target) => target.type === 'page' && target.url.startsWith('http://tauri.localhost'),
);
if (!page) throw new Error('Meetily WebView2 debug target was not found');

const socket = new WebSocket(page.webSocketDebuggerUrl);
let nextId = 1;
const pending = new Map();
socket.addEventListener('message', (event) => {
  const message = JSON.parse(event.data);
  if (!message.id || !pending.has(message.id)) return;
  const { resolve, reject } = pending.get(message.id);
  pending.delete(message.id);
  if (message.error) reject(new Error(JSON.stringify(message.error)));
  else resolve(message.result);
});
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true });
  socket.addEventListener('error', reject, { once: true });
});
function call(method, params = {}) {
  const id = nextId++;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

const install = `(() => {
  if (!window.__MEETILY_QA_RECORDING_PROBE__) {
    const events = [];
    const capture = (kind, detail = null) => events.push({
      at: new Date().toISOString(),
      kind,
      detail,
      href: location.href,
      stack: new Error().stack,
    });
    for (const name of [
      'start-recording-from-sidebar',
      'request-recording-from-tray',
      'request-recording-toggle',
    ]) {
      window.addEventListener(name, (event) => capture('window-event', {
        name,
        detail: event.detail ?? null,
      }), true);
    }
    const originalInvoke = window.__TAURI_INTERNALS__.invoke.bind(window.__TAURI_INTERNALS__);
    window.__TAURI_INTERNALS__.invoke = (command, args, options) => {
      if (command === 'start_recording' || command === 'start_recording_with_devices_and_meeting') {
        capture('start-invoke', { command, args });
      }
      return originalInvoke(command, args, options);
    };
    const originalSetItem = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (key === 'autoStartRecording') capture('storage-set', { value });
      return originalSetItem.call(this, key, value);
    };
    window.__MEETILY_QA_RECORDING_PROBE__ = {
      installedAt: new Date().toISOString(),
      events,
    };
  }
  return window.__MEETILY_QA_RECORDING_PROBE__;
})()`;

const read = `(async () => {
  const probe = window.__MEETILY_QA_RECORDING_PROBE__ ?? null;
  const state = await window.__TAURI_INTERNALS__.invoke('get_recording_state', {});
  return {
    capturedAt: new Date().toISOString(),
    href: location.href,
    pendingAutoStartValue: sessionStorage.getItem('autoStartRecording'),
    state,
    probe,
  };
})()`;

const expression = action === 'install' ? install : read;
const result = await call('Runtime.evaluate', {
  expression,
  returnByValue: true,
  awaitPromise: true,
});
if (result.exceptionDetails) {
  throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
}
console.log(JSON.stringify(result.result.value, null, 2));
socket.close();
