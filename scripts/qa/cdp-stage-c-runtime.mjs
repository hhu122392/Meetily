import fs from "node:fs";
import path from "node:path";

const outputPath = process.argv[2];
if (!outputPath) throw new Error("Usage: node cdp-stage-c-runtime.mjs <report.json>");

const cdpPort = process.env.CDP_PORT ?? "9233";
const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`).then((response) => response.json());
const page = targets.find((target) => target.type === "page" && (
  target.url.startsWith("http://tauri.localhost") || target.url.startsWith("http://localhost:")
));
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

async function invoke(command, payload) {
  const evaluated = await call("Runtime.evaluate", {
    expression: `(async () => {
      try {
        return { ok: true, value: await window.__TAURI_INTERNALS__.invoke(
          ${JSON.stringify(command)}, ${JSON.stringify(payload)}
        ) };
      } catch (error) {
        return { ok: false, error: typeof error === 'string' ? error : JSON.stringify(error) };
      }
    })()`,
    returnByValue: true,
    awaitPromise: true,
  });
  if (evaluated.exceptionDetails) throw new Error(evaluated.exceptionDetails.text);
  if (!evaluated.result.value.ok) throw new Error(`${command}: ${evaluated.result.value.error}`);
  return evaluated.result.value.value;
}

await call("Runtime.enable");
const details = await invoke("api_get_template_v2", {
  request: { templateId: "license_station_weekly" },
});
const template = details.template;
const people = template.extensions?.meetily_meeting_context?.people ?? [];
const expected = new Map([
  ["Rayson", ["瑞森", "Risa", "Reason", "Raison"]],
  ["shunzi", ["顺子"]],
  ["伊犁", ["伊丽"]],
  ["Amu", ["阿牧"]],
  ["jacklee", ["杰克利"]],
]);
const aliases = Object.fromEntries([...expected].map(([name]) => {
  const person = people.find((item) => item.display_name === name);
  return [name, person?.aliases ?? null];
}));
const assertions = {
  templateLoadedByRunningBackend: template.id === "license_station_weekly",
  templateVersionIs3: template.version === 3,
  allExpectedAliasesLoaded: [...expected].every(([name, expectedAliases]) => (
    JSON.stringify(aliases[name]) === JSON.stringify(expectedAliases)
  )),
  ambiguousAliasesNotAdded: people.every((person) => !person.aliases.some((alias) => (
    ["牛", "我牛", "老陆", "老学", "小比", "思域", "黑炭", "阿姨", "粉泥"].includes(alias)
  ))),
};
const failed = Object.entries(assertions).filter(([, passed]) => !passed).map(([name]) => name);
if (failed.length > 0) throw new Error(`Stage C runtime audit failed: ${failed.join(", ")}`);

const report = {
  stage: "C",
  auditedAt: new Date().toISOString(),
  pageUrl: page.url,
  template: {
    id: template.id,
    version: template.version,
    aliases,
  },
  assertions,
  result: "PASS",
};
fs.mkdirSync(path.dirname(outputPath), { recursive: true });
fs.writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
console.log(JSON.stringify(report, null, 2));
socket.close();
