#!/usr/bin/env node

import fs from 'node:fs/promises';
import path from 'node:path';

const port = Number(process.argv[2] || 9222);
const outputDirectory = path.resolve(
  process.argv[3] || 'docs/i18n/audit/phase-0-runtime/cdp',
);

class CdpClient {
  constructor(url) {
    this.url = url;
    this.nextId = 1;
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
      if (message.id) {
        const waiter = this.pending.get(message.id);
        if (!waiter) return;
        this.pending.delete(message.id);
        if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
        else waiter.resolve(message.result);
        return;
      }
      this.events.push(message);
    });
  }

  send(method, params = {}) {
    const id = this.nextId++;
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
      response.exceptionDetails.exception?.description ||
        response.exceptionDetails.text ||
        'Runtime evaluation failed',
    );
  }
  return response.result.value;
}

async function main() {
  await fs.mkdir(outputDirectory, { recursive: true });
  const targets = await fetch(`http://127.0.0.1:${port}/json`).then((response) => {
    if (!response.ok) throw new Error(`CDP target lookup failed: ${response.status}`);
    return response.json();
  });
  const target = targets.find((candidate) => candidate.type === 'page');
  if (!target?.webSocketDebuggerUrl) throw new Error('No debuggable page target found');

  const client = new CdpClient(target.webSocketDebuggerUrl);
  await client.connect();
  await Promise.all([
    client.send('Runtime.enable'),
    client.send('Page.enable'),
    client.send('Log.enable'),
  ]);

  await evaluate(
    client,
    `new Promise((resolve) => {
      if (document.readyState === 'complete') resolve(true);
      else addEventListener('load', () => resolve(true), { once: true });
    })`,
  );

  const snapshot = await evaluate(
    client,
    `(() => {
      const clean = (value) => String(value || '').replace(/\\s+/g, ' ').trim();
      const selector = [
        'button', 'a[href]', 'input', 'textarea', 'select',
        '[role="button"]', '[role="menuitem"]', '[contenteditable="true"]'
      ].join(',');
      const describe = (element, index) => ({
        index,
        tag: element.tagName.toLowerCase(),
        type: element.getAttribute('type'),
        role: element.getAttribute('role'),
        text: clean(element.innerText || element.value),
        ariaLabel: element.getAttribute('aria-label'),
        title: element.getAttribute('title'),
        placeholder: element.getAttribute('placeholder'),
        disabled: Boolean(element.disabled || element.getAttribute('aria-disabled') === 'true'),
        contentEditable: element.isContentEditable,
        rect: (() => {
          const rect = element.getBoundingClientRect();
          return { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
        })(),
      });
      return {
        capturedAt: new Date().toISOString(),
        url: location.href,
        title: document.title,
        readyState: document.readyState,
        htmlLang: document.documentElement.lang,
        bodyText: document.body.innerText,
        hasTauriInternals: Boolean(window.__TAURI_INTERNALS__),
        interactive: [...document.querySelectorAll(selector)].map(describe),
        dialogs: [...document.querySelectorAll('[role="dialog"], dialog')].map(describe),
      };
    })()`,
  );

  const screenshot = await client.send('Page.captureScreenshot', {
    format: 'png',
    captureBeyondViewport: false,
  });
  await fs.writeFile(
    path.join(outputDirectory, 'initial-page.png'),
    Buffer.from(screenshot.data, 'base64'),
  );

  await new Promise((resolve) => setTimeout(resolve, 750));
  const diagnostics = client.events
    .filter((event) =>
      ['Runtime.exceptionThrown', 'Runtime.consoleAPICalled', 'Log.entryAdded'].includes(
        event.method,
      ),
    )
    .map((event) => ({ method: event.method, params: event.params }));

  const report = {
    scope: 'PHASE_0_RUNTIME_PROBE',
    target: {
      id: target.id,
      title: target.title,
      url: target.url,
    },
    snapshot,
    diagnostics,
  };
  await fs.writeFile(
    path.join(outputDirectory, 'initial-page.json'),
    `${JSON.stringify(report, null, 2)}\n`,
    'utf8',
  );
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  client.close();
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
