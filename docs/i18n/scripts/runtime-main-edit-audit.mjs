#!/usr/bin/env node

import fs from 'node:fs/promises';
import path from 'node:path';

const port = Number(process.argv[2] || 9222);
const outputDirectory = path.resolve(
  process.argv[3] || 'docs/i18n/audit/phase-0-runtime/main-edit',
);

class Client {
  constructor(url) {
    this.url = url;
    this.id = 1;
    this.pending = new Map();
    this.events = [];
  }

  async connect() {
    this.socket = new WebSocket(this.url);
    await new Promise((resolve, reject) => {
      this.socket.addEventListener('open', resolve, { once: true });
      this.socket.addEventListener('error', reject, { once: true });
    });
    this.socket.addEventListener('message', (event) => {
      const message = JSON.parse(event.data);
      if (!message.id) {
        this.events.push(message);
        return;
      }
      const waiter = this.pending.get(message.id);
      if (!waiter) return;
      this.pending.delete(message.id);
      if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
      else waiter.resolve(message.result);
    });
  }

  send(method, params = {}) {
    const id = this.id++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    this.socket.close();
  }
}

const sleep = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function evaluate(client, expression) {
  const response = await client.send('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (response.exceptionDetails) {
    throw new Error(
      response.exceptionDetails.exception?.description || response.exceptionDetails.text,
    );
  }
  return response.result.value;
}

async function waitReady(client) {
  await evaluate(
    client,
    `new Promise((resolve) => {
      if (document.readyState === 'complete') resolve(true);
      else addEventListener('load', () => resolve(true), { once: true });
    })`,
  );
  await sleep(1100);
}

async function installTrace(client) {
  await evaluate(
    client,
    `(() => {
      window.__phase0MainTrace = [];
      const types = ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click', 'keydown', 'input', 'change'];
      const make = (phase) => (event) => window.__phase0MainTrace.push({
        type: event.type,
        phase,
        target: event.target?.tagName?.toLowerCase() || null,
        targetText: String(event.target?.innerText || event.target?.value || '').replace(/\\s+/g, ' ').trim().slice(0, 160),
        ariaLabel: event.target?.closest?.('[aria-label]')?.getAttribute('aria-label') || null,
        key: event.key || null,
        bubbles: event.bubbles,
        defaultPrevented: event.defaultPrevented,
      });
      for (const type of types) {
        document.addEventListener(type, make('capture'), true);
        document.addEventListener(type, make('bubble'), false);
      }
      return true;
    })()`,
  );
}

async function center(client, mode, value) {
  return evaluate(
    client,
    `(() => {
      const normalize = (input) => String(input || '').replace(/\\s+/g, ' ').trim();
      let element;
      if (${JSON.stringify(mode)} === 'selector') {
        element = document.querySelector(${JSON.stringify(value)});
      } else if (${JSON.stringify(mode)} === 'text') {
        element = [...document.querySelectorAll('button, a[href], [role="button"]')]
          .find((candidate) => normalize(candidate.innerText) === ${JSON.stringify(value)});
      } else if (${JSON.stringify(mode)} === 'top-left-menu') {
        element = [...document.querySelectorAll('button')].find((candidate) => {
          const rect = candidate.getBoundingClientRect();
          return rect.left < 120 && rect.top >= 60 && rect.top < 120 && rect.width > 20 && rect.height > 20;
        });
      }
      if (!element) return null;
      const rect = element.getBoundingClientRect();
      return {
        x: rect.left + rect.width / 2,
        y: rect.top + rect.height / 2,
        disabled: Boolean(element.disabled),
        tag: element.tagName.toLowerCase(),
      };
    })()`,
  );
}

async function click(client, mode, value) {
  const point = await center(client, mode, value);
  if (!point) throw new Error(`Element not found for ${mode}: ${value}`);
  if (point.disabled) throw new Error(`Element disabled for ${mode}: ${value}`);
  await client.send('Input.dispatchMouseEvent', {
    type: 'mouseMoved',
    x: point.x,
    y: point.y,
    button: 'none',
  });
  await sleep(250);
  await client.send('Input.dispatchMouseEvent', {
    type: 'mousePressed',
    x: point.x,
    y: point.y,
    button: 'left',
    clickCount: 1,
  });
  await client.send('Input.dispatchMouseEvent', {
    type: 'mouseReleased',
    x: point.x,
    y: point.y,
    button: 'left',
    clickCount: 1,
  });
  await sleep(650);
}

async function key(client, key, code, virtualKeyCode, modifiers = 0) {
  await client.send('Input.dispatchKeyEvent', {
    type: 'keyDown',
    key,
    code,
    windowsVirtualKeyCode: virtualKeyCode,
    modifiers,
  });
  await client.send('Input.dispatchKeyEvent', {
    type: 'keyUp',
    key,
    code,
    windowsVirtualKeyCode: virtualKeyCode,
    modifiers,
  });
}

async function replaceFocusedText(client, text) {
  await client.send('Input.dispatchKeyEvent', {
    type: 'keyDown',
    key: 'Control',
    code: 'ControlLeft',
    windowsVirtualKeyCode: 17,
    modifiers: 2,
  });
  await key(client, 'a', 'KeyA', 65, 2);
  await client.send('Input.dispatchKeyEvent', {
    type: 'keyUp',
    key: 'Control',
    code: 'ControlLeft',
    windowsVirtualKeyCode: 17,
  });
  await client.send('Input.insertText', { text });
  await sleep(350);
}

async function snapshot(client, name) {
  const data = await evaluate(
    client,
    `(() => ({
      name: ${JSON.stringify(name)},
      capturedAt: new Date().toISOString(),
      url: location.href,
      htmlLang: document.documentElement.lang,
      bodyText: document.body.innerText,
      dialogCount: document.querySelectorAll('[role="dialog"], dialog').length,
      inputValues: [...document.querySelectorAll('input, textarea, [contenteditable="true"]')].map((element) => ({
        id: element.id || null,
        placeholder: element.getAttribute('placeholder'),
        value: element.value ?? element.textContent,
      })),
      trace: [...(window.__phase0MainTrace || [])],
    }))()`,
  );
  const image = await client.send('Page.captureScreenshot', {
    format: 'png',
    captureBeyondViewport: false,
  });
  await Promise.all([
    fs.writeFile(path.join(outputDirectory, `${name}.json`), `${JSON.stringify(data, null, 2)}\n`),
    fs.writeFile(path.join(outputDirectory, `${name}.png`), Buffer.from(image.data, 'base64')),
  ]);
  return data;
}

async function main() {
  await fs.mkdir(outputDirectory, { recursive: true });
  const targets = await fetch(`http://127.0.0.1:${port}/json`).then((response) => response.json());
  const target = targets.find((candidate) => candidate.type === 'page');
  if (!target?.webSocketDebuggerUrl) throw new Error('No page target found');
  const client = new Client(target.webSocketDebuggerUrl);
  await client.connect();
  await Promise.all([
    client.send('Runtime.enable'),
    client.send('Page.enable'),
    client.send('Log.enable'),
  ]);
  await client.send('Page.navigate', { url: 'http://tauri.localhost/' });
  await waitReady(client);
  const preflightDialogCount = await evaluate(
    client,
    `document.querySelectorAll('[role="dialog"], dialog').length`,
  );
  if (preflightDialogCount > 0) {
    await key(client, 'Escape', 'Escape', 27);
    await sleep(400);
    const remainingDialogs = await evaluate(
      client,
      `document.querySelectorAll('[role="dialog"], dialog').length`,
    );
    if (remainingDialogs !== 0) {
      throw new Error(`Escape did not close preflight dialog; remaining=${remainingDialogs}`);
    }
  }
  await installTrace(client);

  const report = {
    scope: 'PHASE_0_STRICT_MAIN_EDIT_DIALOG_AUDIT',
    startedAt: new Date().toISOString(),
    preflightDialogCount,
    checks: [],
  };

  await click(client, 'top-left-menu', '');
  const expanded = await snapshot(client, '01-sidebar-expanded');
  report.checks.push({
    id: 'NAV-SIDEBAR-EXPAND',
    pass:
      expanded.bodyText.includes('Meeting Notes') &&
      expanded.bodyText.includes('Phase 0 Synthetic Meeting'),
    evidence: '01-sidebar-expanded.json',
  });

  await click(client, 'selector', '[aria-label="Edit meeting title"]');
  const dialog = await snapshot(client, '02-edit-dialog-open');
  const editClick = dialog.trace.filter(
    (entry) => entry.type === 'click' && entry.ariaLabel === 'Edit meeting title',
  );
  report.checks.push({
    id: 'DIALOG-EDIT-PARENT-NAVIGATION-GUARD',
    pass:
      dialog.dialogCount === 1 &&
      dialog.bodyText.includes('Edit Meeting Title') &&
      editClick.some((entry) => entry.phase === 'capture') &&
      editClick.some((entry) => entry.phase === 'bubble') &&
      dialog.url === 'http://tauri.localhost/',
    evidence: '02-edit-dialog-open.json',
    observedPhases: editClick.map((entry) => entry.phase),
    expectedReason:
      'The nested edit control is observed by document capture/bubble instrumentation, while React stopPropagation prevents the parent meeting row from navigating.',
  });

  await click(client, 'selector', '#meeting-title');
  await replaceFocusedText(client, 'Phase 0 Edited Meeting');
  const editedInput = await snapshot(client, '03-title-input-edited');
  const inputEvents = editedInput.trace.filter(
    (entry) => entry.type === 'input' && entry.target === 'input',
  );
  report.checks.push({
    id: 'EDIT-REACT-CONTROLLED-INPUT',
    pass:
      editedInput.inputValues.some(
        (input) => input.id === 'meeting-title' && input.value === 'Phase 0 Edited Meeting',
      ) &&
      inputEvents.some((entry) => entry.phase === 'capture') &&
      inputEvents.some((entry) => entry.phase === 'bubble'),
    evidence: '03-title-input-edited.json',
    observedPhases: inputEvents.map((entry) => entry.phase),
  });

  await key(client, 'Enter', 'Enter', 13);
  await sleep(1000);
  const savedMeetings = await evaluate(
    client,
    `window.__TAURI_INTERNALS__.invoke('api_get_meetings')`,
  );
  const saved = await snapshot(client, '04-title-saved-with-enter');
  const enterEvents = saved.trace.filter(
    (entry) => entry.type === 'keydown' && entry.key === 'Enter',
  );
  report.checks.push({
    id: 'EDIT-ENTER-SAVE-RUST-ROUNDTRIP',
    pass:
      saved.dialogCount === 0 &&
      savedMeetings.some(
        (meeting) =>
          meeting.id === 'phase0-audit-meeting' &&
          meeting.title === 'Phase 0 Edited Meeting',
      ) &&
      enterEvents.some((entry) => entry.phase === 'capture') &&
      enterEvents.some((entry) => entry.phase === 'bubble'),
    evidence: '04-title-saved-with-enter.json',
    backendMeetings: savedMeetings,
  });

  await click(client, 'selector', '[aria-label="Edit meeting title"]');
  await click(client, 'selector', '#meeting-title');
  await replaceFocusedText(client, 'Unsaved Phase 0 Title');
  await key(client, 'Escape', 'Escape', 27);
  await sleep(500);
  const afterEscapeMeetings = await evaluate(
    client,
    `window.__TAURI_INTERNALS__.invoke('api_get_meetings')`,
  );
  const cancelled = await snapshot(client, '05-title-edit-cancelled-with-escape');
  report.checks.push({
    id: 'EDIT-ESCAPE-CANCEL-NO-PERSIST',
    pass:
      cancelled.dialogCount === 0 &&
      afterEscapeMeetings.some(
        (meeting) =>
          meeting.id === 'phase0-audit-meeting' &&
          meeting.title === 'Phase 0 Edited Meeting',
      ),
    evidence: '05-title-edit-cancelled-with-escape.json',
    backendMeetings: afterEscapeMeetings,
  });

  await click(client, 'text', 'Settings');
  const settings = await snapshot(client, '06-settings-navigation');
  report.checks.push({
    id: 'NAV-SETTINGS-ENGLISH-SMOKE',
    pass:
      settings.bodyText.includes('Settings') &&
      /Transcription|Summary|Recording|Notification/.test(settings.bodyText),
    evidence: '06-settings-navigation.json',
  });

  const diagnostics = client.events.filter((event) =>
    ['Runtime.exceptionThrown', 'Runtime.consoleAPICalled', 'Log.entryAdded'].includes(event.method),
  );
  const runtimeExceptions = diagnostics.filter(
    (event) => event.method === 'Runtime.exceptionThrown',
  );
  await fs.writeFile(
    path.join(outputDirectory, '07-diagnostics.json'),
    `${JSON.stringify(diagnostics, null, 2)}\n`,
  );
  report.diagnostics = {
    total: diagnostics.length,
    runtimeExceptions: runtimeExceptions.length,
    evidence: '07-diagnostics.json',
  };
  report.finishedAt = new Date().toISOString();
  report.summary = {
    passed: report.checks.filter((check) => check.pass).length,
    failed: report.checks.filter((check) => !check.pass).length,
    total: report.checks.length,
  };
  await fs.writeFile(
    path.join(outputDirectory, 'main-edit-report.json'),
    `${JSON.stringify(report, null, 2)}\n`,
  );
  client.close();
  process.stdout.write(`${JSON.stringify(report.summary)}\n`);
  if (report.summary.failed > 0) process.exitCode = 1;
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exit(1);
});
