const action = process.argv[2];
if (!new Set(["dismiss-recovery", "start", "pause", "resume"]).has(action)) {
  throw new Error("Usage: node cdp-recording-ui-control.mjs <dismiss-recovery|start|pause|resume>");
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
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const action = ${JSON.stringify(action)};
  const visibleButtons = () => [...document.querySelectorAll('button')].filter((button) => {
    const rect = button.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0 && !button.disabled;
  });
  const before = await invoke('get_recording_state', {});
  if (action === 'dismiss-recovery') {
    const buttons = visibleButtons().filter((button) => button.textContent.trim() === '取消');
    if (buttons.length !== 1) throw new Error('Expected exactly one visible recovery Cancel button');
    buttons[0].click();
    await sleep(500);
    return {
      action,
      clickedAt: new Date().toISOString(),
      buttonCount: buttons.length,
      before,
      after: await invoke('get_recording_state', {}),
      visibleText: document.body.innerText.slice(0, 1000),
    };
  }

  if (action === 'pause' || action === 'resume') {
    const paused = action === 'pause';
    if (!before.is_recording || before.is_paused === paused) {
      throw new Error('Recording is not in the expected pause/resume state');
    }
    const label = paused ? '暂停录音' : '继续录音';
    const buttons = visibleButtons().filter(button => button.getAttribute('aria-label') === label);
    if (buttons.length !== 1) throw new Error('Expected exactly one enabled pause/resume UI button');
    const clickedAt = new Date().toISOString();
    buttons[0].click();
    const samples = [];
    const deadline = performance.now() + 10000;
    while (performance.now() < deadline) {
      const state = await invoke('get_recording_state', {});
      samples.push({ at: new Date().toISOString(), state });
      if (state.is_recording && state.is_paused === paused && state.is_active === !paused) {
        return { action, clickedAt, label, before, after: state, samples, verdict: { stateChanged: true } };
      }
      await sleep(100);
    }
    throw new Error('Pause/resume UI did not reach expected backend state');
  }

  if (before.is_recording || before.is_active) {
    throw new Error('Refusing to start because the backend is already recording');
  }
  const buttons = visibleButtons().filter((button) => {
    const text = button.textContent.trim();
    const label = button.getAttribute('aria-label') ?? '';
    return text.includes('开始录音') || label.includes('开始录音');
  });
  if (buttons.length < 1) {
    throw new Error('Expected at least one enabled Start Recording button');
  }
  // The welcome page intentionally exposes both the central call-to-action
  // and the persistent sidebar action. Click exactly one deterministic UI
  // control: the lowest visible button is the sidebar action used during the
  // rest of the real acceptance flow.
  const candidates = buttons.map((button) => {
    const rect = button.getBoundingClientRect();
    return {
      button,
      text: button.textContent.trim(),
      ariaLabel: button.getAttribute('aria-label'),
      rect: { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom },
    };
  }).sort((left, right) => right.rect.bottom - left.rect.bottom);
  const selected = candidates[0];
  const clickedAt = new Date().toISOString();
  selected.button.click();
  const samples = [];
  const deadline = performance.now() + 15000;
  while (performance.now() < deadline) {
    const state = await invoke('get_recording_state', {});
    samples.push({ at: new Date().toISOString(), state });
    if (state.is_recording && state.is_active) break;
    await sleep(200);
  }
  const after = await invoke('get_recording_state', {});
  return {
    action,
    clickedAt,
    buttonCount: buttons.length,
    candidates: candidates.map(({ text, ariaLabel, rect }) => ({ text, ariaLabel, rect })),
    selected: { text: selected.text, ariaLabel: selected.ariaLabel, rect: selected.rect },
    before,
    after,
    folder: await invoke('get_meeting_folder_path', {}).catch(() => null),
    meetingName: await invoke('get_current_meeting_name', {}).catch(() => null),
    samples,
    verdict: {
      oneStartClick: true,
      backendStarted: after.is_recording === true && after.is_active === true,
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
console.log(JSON.stringify(evaluated.result.value, null, 2));
socket.close();
