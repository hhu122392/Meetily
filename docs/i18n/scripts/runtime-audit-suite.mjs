#!/usr/bin/env node

import fs from 'node:fs/promises';
import path from 'node:path';

const port = Number(process.argv[2] || 9222);
const outputDirectory = path.resolve(
  process.argv[3] || 'docs/i18n/audit/phase-0-runtime/suite',
);

class CdpClient {
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
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(JSON.stringify(message.error)));
      else pending.resolve(message.result);
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

const sleep = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function installEventTrace(client) {
  return evaluate(
    client,
    `(() => {
      window.__phase0EventTrace = [];
      const types = ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click', 'keydown', 'input', 'change'];
      const record = (phase) => (event) => window.__phase0EventTrace.push({
        type: event.type,
        phase,
        eventPhase: event.eventPhase,
        target: event.target?.tagName?.toLowerCase() || null,
        targetText: String(event.target?.innerText || event.target?.value || '').replace(/\\s+/g, ' ').trim().slice(0, 120),
        key: event.key || null,
        bubbles: event.bubbles,
        composed: event.composed,
        defaultPrevented: event.defaultPrevented,
        timestamp: performance.now(),
      });
      for (const type of types) {
        document.addEventListener(type, record('capture'), true);
        document.addEventListener(type, record('bubble'), false);
      }
      return types;
    })()`,
  );
}

async function elementCenterByText(client, text) {
  const literal = JSON.stringify(text);
  return evaluate(
    client,
    `(() => {
      const normalize = (value) => String(value || '').replace(/\\s+/g, ' ').trim();
      const candidates = [...document.querySelectorAll('button, a[href], [role="button"]')];
      const element = candidates.find((candidate) => normalize(candidate.innerText) === ${literal});
      if (!element) return null;
      const rect = element.getBoundingClientRect();
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2, tag: element.tagName.toLowerCase(), disabled: Boolean(element.disabled) };
    })()`,
  );
}

async function realMouseClick(client, text) {
  const center = await elementCenterByText(client, text);
  if (!center) throw new Error(`Visible interactive element not found: ${text}`);
  if (center.disabled) throw new Error(`Interactive element is disabled: ${text}`);
  await client.send('Input.dispatchMouseEvent', {
    type: 'mouseMoved',
    x: center.x,
    y: center.y,
    button: 'none',
  });
  await client.send('Input.dispatchMouseEvent', {
    type: 'mousePressed',
    x: center.x,
    y: center.y,
    button: 'left',
    clickCount: 1,
  });
  await client.send('Input.dispatchMouseEvent', {
    type: 'mouseReleased',
    x: center.x,
    y: center.y,
    button: 'left',
    clickCount: 1,
  });
  await sleep(600);
  return center;
}

async function realMouseClickFirstEmptyButton(client) {
  const center = await evaluate(
    client,
    `(() => {
      const element = [...document.querySelectorAll('button')].find((candidate) =>
        !candidate.disabled && !String(candidate.innerText || '').trim()
      );
      if (!element) return null;
      const rect = element.getBoundingClientRect();
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    })()`,
  );
  if (!center) throw new Error('No enabled empty navigation button found');
  await client.send('Input.dispatchMouseEvent', {
    type: 'mousePressed',
    x: center.x,
    y: center.y,
    button: 'left',
    clickCount: 1,
  });
  await client.send('Input.dispatchMouseEvent', {
    type: 'mouseReleased',
    x: center.x,
    y: center.y,
    button: 'left',
    clickCount: 1,
  });
  await sleep(700);
}

async function snapshot(client, name) {
  const data = await evaluate(
    client,
    `(() => {
      const normalize = (value) => String(value || '').replace(/\\s+/g, ' ').trim();
      const selector = 'button, a[href], input, textarea, select, [role="button"], [role="dialog"], [contenteditable="true"]';
      return {
        name: ${JSON.stringify(name)},
        capturedAt: new Date().toISOString(),
        url: location.href,
        htmlLang: document.documentElement.lang,
        bodyText: document.body.innerText,
        activeElement: {
          tag: document.activeElement?.tagName?.toLowerCase() || null,
          text: normalize(document.activeElement?.innerText || document.activeElement?.value),
        },
        interactive: [...document.querySelectorAll(selector)].map((element, index) => ({
          index,
          tag: element.tagName.toLowerCase(),
          text: normalize(element.innerText || element.value),
          title: element.getAttribute('title'),
          ariaLabel: element.getAttribute('aria-label'),
          placeholder: element.getAttribute('placeholder'),
          role: element.getAttribute('role'),
          disabled: Boolean(element.disabled || element.getAttribute('aria-disabled') === 'true'),
          contentEditable: element.isContentEditable,
        })),
        eventTrace: [...(window.__phase0EventTrace || [])],
      };
    })()`,
  );
  const screenshot = await client.send('Page.captureScreenshot', {
    format: 'png',
    captureBeyondViewport: false,
  });
  await Promise.all([
    fs.writeFile(path.join(outputDirectory, `${name}.json`), `${JSON.stringify(data, null, 2)}\n`),
    fs.writeFile(path.join(outputDirectory, `${name}.png`), Buffer.from(screenshot.data, 'base64')),
  ]);
  return data;
}

async function waitForReady(client) {
  await evaluate(
    client,
    `new Promise((resolve) => {
      if (document.readyState === 'complete') resolve(true);
      else addEventListener('load', () => resolve(true), { once: true });
    })`,
  );
  await sleep(1200);
}

async function main() {
  await fs.mkdir(outputDirectory, { recursive: true });
  const targets = await fetch(`http://127.0.0.1:${port}/json`).then((response) => response.json());
  const target = targets.find((candidate) => candidate.type === 'page');
  if (!target?.webSocketDebuggerUrl) throw new Error('No page target available');
  const client = new CdpClient(target.webSocketDebuggerUrl);
  await client.connect();
  await Promise.all([
    client.send('Runtime.enable'),
    client.send('Page.enable'),
    client.send('Log.enable'),
  ]);
  await waitForReady(client);

  const preflightStates = [];
  for (let attempt = 0; attempt < 3; attempt += 1) {
    const bodyText = await evaluate(client, 'document.body.innerText');
    preflightStates.push(bodyText);
    if (bodyText.includes('Welcome to Meetily')) break;
    if (
      bodyText.includes('Getting things ready') ||
      bodyText.includes('Setup Overview')
    ) {
      await realMouseClickFirstEmptyButton(client);
      continue;
    }
    throw new Error(`Unsupported preflight UI state: ${bodyText.slice(0, 240)}`);
  }
  const stabilizedBody = await evaluate(client, 'document.body.innerText');
  if (!stabilizedBody.includes('Welcome to Meetily')) {
    throw new Error(`Failed to return onboarding to Welcome: ${stabilizedBody.slice(0, 240)}`);
  }
  await installEventTrace(client);

  const report = {
    scope: 'PHASE_0_STRICT_RUNTIME_UI_AND_PROPAGATION_AUDIT',
    startedAt: new Date().toISOString(),
    preflightStates,
    checks: [],
  };

  const welcome = await snapshot(client, '01-welcome-before-click');
  report.checks.push({
    id: 'UI-WELCOME-EN',
    pass: welcome.htmlLang === 'en' && welcome.bodyText.includes('Welcome to Meetily'),
    evidence: '01-welcome-before-click.json',
  });

  await realMouseClick(client, 'Get Started');
  const setup = await snapshot(client, '02-setup-after-real-click');
  const getStartedClick = setup.eventTrace.filter(
    (entry) => entry.type === 'click' && entry.targetText === 'Get Started',
  );
  report.checks.push({
    id: 'EVENT-REAL-MOUSE-CAPTURE-BUBBLE',
    pass:
      setup.bodyText.includes('Setup Overview') &&
      getStartedClick.some((entry) => entry.phase === 'capture') &&
      getStartedClick.some((entry) => entry.phase === 'bubble'),
    evidence: '02-setup-after-real-click.json',
    observedPhases: getStartedClick.map((entry) => entry.phase),
  });

  await evaluate(
    client,
    `(() => {
      const button = [...document.querySelectorAll('button')].find((element) => !String(element.innerText || '').trim());
      if (!button) return false;
      button.focus();
      return true;
    })()`,
  );
  await client.send('Input.dispatchKeyEvent', {
    type: 'keyDown',
    key: 'Tab',
    code: 'Tab',
    windowsVirtualKeyCode: 9,
  });
  await client.send('Input.dispatchKeyEvent', {
    type: 'keyUp',
    key: 'Tab',
    code: 'Tab',
    windowsVirtualKeyCode: 9,
  });
  await sleep(250);
  const keyboard = await snapshot(client, '03-setup-keyboard-tab');
  const tabEvents = keyboard.eventTrace.filter(
    (entry) => entry.type === 'keydown' && entry.key === 'Tab',
  );
  report.checks.push({
    id: 'EVENT-KEYBOARD-CAPTURE-BUBBLE',
    pass:
      tabEvents.some((entry) => entry.phase === 'capture') &&
      tabEvents.some((entry) => entry.phase === 'bubble') &&
      keyboard.activeElement.tag !== 'body',
    evidence: '03-setup-keyboard-tab.json',
    activeElement: keyboard.activeElement,
  });

  const ipc = await evaluate(
    client,
    `(async () => {
      const invoke = window.__TAURI_INTERNALS__?.invoke;
      if (typeof invoke !== 'function') return { available: false };
      const result = { available: true };
      try {
        result.validBefore = await invoke('get_onboarding_status');
        result.validCommandResolved = true;
      } catch (error) {
        result.validCommandResolved = false;
        result.validCommandError = String(error);
      }
      try {
        await invoke('__phase0_nonexistent_command__');
        result.unknownCommandRejected = false;
      } catch (error) {
        result.unknownCommandRejected = true;
        result.unknownCommandError = String(error);
      }
      try {
        await invoke('api_save_meeting_title', {
          meetingId: '__phase0_missing_meeting__',
          title: 'Phase 0 audit title',
        });
        result.nativeDomainFailureRejected = false;
      } catch (error) {
        result.nativeDomainFailureRejected = true;
        result.nativeDomainFailureError = String(error);
      }
      return result;
    })()`,
  );
  await fs.writeFile(
    path.join(outputDirectory, '04-ipc-error-propagation.json'),
    `${JSON.stringify(ipc, null, 2)}\n`,
  );
  report.checks.push({
    id: 'ERROR-RUST-TAURI-JS-REJECTION',
    pass:
      ipc.available === true &&
      ipc.validCommandResolved === true &&
      ipc.unknownCommandRejected === true &&
      ipc.nativeDomainFailureRejected === true &&
      /No meeting found/.test(ipc.nativeDomainFailureError || ''),
    evidence: '04-ipc-error-propagation.json',
    details: ipc,
  });

  const completion = await evaluate(
    client,
    `(async () => {
      const status = {
        version: '1.0',
        completed: true,
        current_step: 4,
        model_status: {
          parakeet: 'not_downloaded',
          summary: 'not_downloaded',
          selected_summary_model: 'Qwen3.5-2B-Q4_K_M.gguf',
        },
        last_updated: new Date().toISOString(),
      };
      await window.__TAURI_INTERNALS__.invoke('save_onboarding_status_cmd', { status });
      return window.__TAURI_INTERNALS__.invoke('get_onboarding_status');
    })()`,
  );
  await fs.writeFile(
    path.join(outputDirectory, '05-isolated-onboarding-bypass.json'),
    `${JSON.stringify(completion, null, 2)}\n`,
  );
  await client.send('Page.reload', { ignoreCache: true });
  await waitForReady(client);
  await installEventTrace(client);
  const mainPage = await snapshot(client, '06-main-after-isolated-status');
  report.checks.push({
    id: 'UI-MAIN-ENGLISH-SMOKE',
    pass:
      mainPage.htmlLang === 'en' &&
      mainPage.bodyText.includes('Welcome to meetily!') &&
      !mainPage.bodyText.includes('Welcome to Meetily\n\nRecord. Transcribe.'),
    evidence: '06-main-after-isolated-status.json',
  });

  const diagnosticEvents = client.events.filter((event) =>
    ['Runtime.exceptionThrown', 'Runtime.consoleAPICalled', 'Log.entryAdded'].includes(event.method),
  );
  const runtimeExceptions = diagnosticEvents.filter(
    (event) => event.method === 'Runtime.exceptionThrown',
  );
  const cspIpcEntries = diagnosticEvents.filter(
    (event) => JSON.stringify(event.params).includes('ipc.localhost'),
  );
  await fs.writeFile(
    path.join(outputDirectory, '07-runtime-diagnostics.json'),
    `${JSON.stringify(diagnosticEvents, null, 2)}\n`,
  );
  report.diagnostics = {
    total: diagnosticEvents.length,
    runtimeExceptions: runtimeExceptions.length,
    cspIpcEntries: cspIpcEntries.length,
    cspFallbackIssueDetected: cspIpcEntries.length > 0,
    evidence: '07-runtime-diagnostics.json',
  };
  report.finishedAt = new Date().toISOString();
  report.summary = {
    passed: report.checks.filter((check) => check.pass).length,
    failed: report.checks.filter((check) => !check.pass).length,
    total: report.checks.length,
  };
  await fs.writeFile(
    path.join(outputDirectory, 'runtime-suite-report.json'),
    `${JSON.stringify(report, null, 2)}\n`,
  );
  process.stdout.write(`${JSON.stringify(report.summary)}\n`);
  client.close();
  if (report.summary.failed > 0) process.exitCode = 1;
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exit(1);
});
