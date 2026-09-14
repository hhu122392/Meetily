#!/usr/bin/env node

const port = Number(process.argv[2]);
const expression = process.argv[3];

if (!port || !expression) {
  throw new Error("Usage: cdp-eval.mjs <port> <javascript-expression>");
}

const targets = await fetch(`http://127.0.0.1:${port}/json`).then((response) =>
  response.json(),
);
const target = targets.find((candidate) => candidate.type === "page");
if (!target?.webSocketDebuggerUrl) throw new Error("No page target");

const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener("open", resolve, { once: true });
  socket.addEventListener("error", reject, { once: true });
});

const response = await new Promise((resolve, reject) => {
  socket.addEventListener(
    "message",
    (event) => {
      const message = JSON.parse(event.data);
      if (message.id !== 1) return;
      if (message.error) reject(new Error(JSON.stringify(message.error)));
      else resolve(message.result);
    },
    { once: false },
  );
  socket.send(
    JSON.stringify({
      id: 1,
      method: "Runtime.evaluate",
      params: {
        expression,
        awaitPromise: true,
        returnByValue: true,
        userGesture: true,
      },
    }),
  );
});

socket.close();
if (response.exceptionDetails) {
  throw new Error(
    response.exceptionDetails.exception?.description ||
      response.exceptionDetails.text ||
      "Runtime evaluation failed",
  );
}
process.stdout.write(`${JSON.stringify(response.result.value)}\n`);
