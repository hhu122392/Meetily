import fs from "node:fs";
import path from "node:path";

const destinationPath = process.argv[2];
const reportPath = process.argv[3];
if (!destinationPath || !reportPath) {
  throw new Error("Usage: node cdp-export-company-template-pack.mjs <pack-path> <report-path>");
}

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
const templateIds = [
  "company_monthly_review",
  "license_station_weekly",
  "performance_interview_record",
  "quarterly_performance_review_report",
];
const expectedVersions = new Map([
  ["company_monthly_review", 2],
  ["license_station_weekly", 3],
  ["performance_interview_record", 2],
  ["quarterly_performance_review_report", 2],
]);
const preview = await invoke("api_preview_template_pack_export", {
  request: { templateIds },
});
if (preview.templateCount !== 4) throw new Error(`Expected 4 templates, got ${preview.templateCount}`);
for (const template of preview.templates) {
  const expectedVersion = expectedVersions.get(template.id);
  if (template.version !== expectedVersion) {
    throw new Error(
      `Unexpected ${template.id} version: expected ${expectedVersion}, got ${template.version}`,
    );
  }
}
const exported = await invoke("api_export_template_pack", {
  request: {
    planToken: preview.planToken,
    destinationPath: path.resolve(destinationPath),
    overwrite: true,
  },
});
const importPreview = await invoke("api_preview_template_pack_import", {
  request: { sourcePath: path.resolve(destinationPath) },
});
if (importPreview.items?.length !== 4) {
  throw new Error(`Exported pack re-open returned ${importPreview.items?.length ?? 0} templates`);
}

const report = {
  exportedAt: new Date().toISOString(),
  destinationPath: path.resolve(destinationPath),
  byteSize: fs.statSync(destinationPath).size,
  archiveSha256: exported.package.archiveSha256,
  templateCount: exported.package.templateCount,
  templates: exported.exportedTemplates,
  reopenedTemplateCount: importPreview.items.length,
  warnings: preview.warnings,
  result: "PASS",
};
fs.mkdirSync(path.dirname(reportPath), { recursive: true });
fs.writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
console.log(JSON.stringify(report, null, 2));
socket.close();
