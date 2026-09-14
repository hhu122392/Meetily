import fs from "node:fs";
import path from "node:path";

const outputPath = process.argv[2];
const templateId = process.argv[3] ?? "license_station_weekly";
if (!outputPath) {
  throw new Error(
    "Usage: node cdp-update-license-station-recognition-aliases.mjs <output.json> [template-id]",
  );
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

async function evaluate(expression) {
  const evaluated = await call("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
    userGesture: true,
  });
  if (evaluated.exceptionDetails) {
    throw new Error(evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text);
  }
  return evaluated.result.value;
}

async function invoke(command, payload = {}) {
  const result = await evaluate(`(async () => {
    try {
      return { ok: true, value: await window.__TAURI_INTERNALS__.invoke(
        ${JSON.stringify(command)}, ${JSON.stringify(payload)}
      ) };
    } catch (error) {
      return { ok: false, error: typeof error === 'string' ? error : JSON.stringify(error) };
    }
  })()`);
  if (!result.ok) throw new Error(`${command}: ${result.error}`);
  return result.value;
}

function addAliases(items, idKey, additions) {
  const changes = [];
  for (const item of items) {
    const aliases = additions[item[idKey]];
    if (!aliases) continue;
    const before = Array.isArray(item.aliases) ? [...item.aliases] : [];
    item.aliases = [...new Set([...before, ...aliases])];
    changes.push({ id: item[idKey], before, after: [...item.aliases] });
  }
  return changes;
}

await call("Runtime.enable");
const startedAt = new Date().toISOString();
const current = await invoke("api_get_template_v2", {
  request: { templateId },
});
if (current.readOnly || current.origin !== "custom") {
  throw new Error(`Template ${templateId} is not an editable custom template`);
}
if (current.template.version !== 3) {
  throw new Error(
    `Refusing stale update: expected version 3, got ${current.template.version}`,
  );
}

const template = structuredClone(current.template);
const profile = template.extensions?.meetily_meeting_context;
if (!profile || !Array.isArray(profile.people) || !Array.isArray(profile.terms)) {
  throw new Error("Template does not contain a valid meeting-context profile");
}

const termChanges = addAliases(profile.terms, "term_id", {
  term_youtube: ["U2B"],
  term_pwa: ["PW"],
});
const personChanges = addAliases(profile.people, "person_id", {
  person_amu: ["阿木"],
  person_yili: ["异利"],
  person_luzhen: ["老卢"],
});
if (termChanges.length !== 2 || personChanges.length !== 3) {
  throw new Error(
    `Expected 2 term and 3 person targets, got ${termChanges.length} and ${personChanges.length}`,
  );
}

const validation = await invoke("api_validate_template_v2", {
  request: { template, mode: "update" },
});
if (!validation.valid || !validation.normalized) {
  throw new Error(`Template validation failed: ${JSON.stringify(validation)}`);
}

const updated = await invoke("api_update_template", {
  request: {
    templateId: current.template.id,
    expectedVersion: current.template.version,
    expectedFileSha256: current.fileSha256,
    template: validation.normalized,
  },
});

const report = {
  startedAt,
  finishedAt: new Date().toISOString(),
  href: page.url,
  templateId,
  before: {
    version: current.template.version,
    fileSha256: current.fileSha256,
  },
  requestedChanges: { terms: termChanges, people: personChanges },
  validation: {
    valid: validation.valid,
    errors: validation.errors,
    warnings: validation.warnings,
  },
  after: {
    version: updated.template.version,
    fileSha256: updated.fileSha256,
    terms: updated.template.extensions.meetily_meeting_context.terms,
    people: updated.template.extensions.meetily_meeting_context.people,
  },
  verdict: {
    versionIncrementedOnce: updated.template.version === current.template.version + 1,
    fileSha256Changed: updated.fileSha256 !== current.fileSha256,
    validationPassed: validation.valid === true,
    expectedTermAliasesAdded: [
      ["term_youtube", "U2B"],
      ["term_pwa", "PW"],
    ].every(([id, alias]) =>
      updated.template.extensions.meetily_meeting_context.terms
        .find((term) => term.term_id === id)?.aliases.includes(alias),
    ),
    expectedPersonAliasesAdded: [
      ["person_amu", "阿木"],
      ["person_yili", "异利"],
      ["person_luzhen", "老卢"],
    ].every(([id, alias]) =>
      updated.template.extensions.meetily_meeting_context.people
        .find((person) => person.person_id === id)?.aliases.includes(alias),
    ),
  },
};

if (Object.values(report.verdict).some((value) => value !== true)) {
  throw new Error(`Post-update audit failed: ${JSON.stringify(report.verdict)}`);
}
fs.mkdirSync(path.dirname(outputPath), { recursive: true });
fs.writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
console.log(JSON.stringify(report, null, 2));
socket.close();
