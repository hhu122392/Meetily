#!/usr/bin/env node

const port = Number(process.argv[2] || 9222);
const command = process.argv[3];
const args = JSON.parse(process.argv[4] || '{}');

if (!command) throw new Error('Usage: cdp-tauri-invoke.mjs <port> <command> [json-args]');

const targets = await fetch(`http://127.0.0.1:${port}/json`).then((response) => response.json());
const target = targets.find((candidate) => candidate.type === 'page');
if (!target?.webSocketDebuggerUrl) throw new Error('No page target found');

const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true });
  socket.addEventListener('error', reject, { once: true });
});

const expression = `(async () => {
  try {
    const value = await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)});
    return { resolved: true, value };
  } catch (error) {
    return { resolved: false, error: String(error) };
  }
})()`;

const result = await new Promise((resolve, reject) => {
  const id = 1;
  socket.addEventListener('message', (event) => {
    const message = JSON.parse(event.data);
    if (message.id !== id) return;
    if (message.error) reject(new Error(JSON.stringify(message.error)));
    else resolve(message.result);
  });
  socket.send(
    JSON.stringify({
      id,
      method: 'Runtime.evaluate',
      params: { expression, awaitPromise: true, returnByValue: true, userGesture: true },
    }),
  );
});

socket.close();
if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
process.stdout.write(`${JSON.stringify(result.result.value, null, 2)}\n`);
