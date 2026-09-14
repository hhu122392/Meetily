#!/usr/bin/env node

const port = Number(process.argv[2]);
const command = process.argv[3];
const args = process.argv[4] ? JSON.parse(process.argv[4]) : {};

if (!port || !command) {
  throw new Error("Usage: cdp-invoke.mjs <port> <command> [json-args]");
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
  );
  const expression = `(async()=>{try{return {resolved:true,value:await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)},${JSON.stringify(args)})}}catch(error){return {resolved:false,error:String(error)}}})()`;
  socket.send(
    JSON.stringify({
      id: 1,
      method: "Runtime.evaluate",
      params: { expression, awaitPromise: true, returnByValue: true },
    }),
  );
});

socket.close();
if (response.exceptionDetails) {
  throw new Error(response.exceptionDetails.text || "Runtime evaluation failed");
}
process.stdout.write(`${JSON.stringify(response.result.value)}\n`);
