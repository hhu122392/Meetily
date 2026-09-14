import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const outputPath = process.argv[2];
const templateId = process.argv[3] ?? "license_station_weekly";
const meetingNamePrefix = process.argv[4] ?? "UAT真实业务会议-牌照站周会";
const expectedTemplateVersion = Number(process.argv[5] ?? "4");
if (!outputPath) {
  throw new Error(
    "Usage: node cdp-uat-real-business-start.mjs <output.json> [template-id] [meeting-name-prefix] [expected-version]",
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

function canonicalize(value) {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, item]) => [key, canonicalize(item)]),
    );
  }
  return value;
}

function sha256Json(value) {
  return crypto.createHash("sha256").update(JSON.stringify(canonicalize(value)), "utf8").digest("hex");
}

function normalizeProfile(profile) {
  const optional = (value) => {
    const trimmed = typeof value === "string" ? value.trim() : "";
    return trimmed || null;
  };
  return {
    schema_version: 1,
    fixed_meeting_mechanism: optional(profile.fixed_meeting_mechanism),
    people: profile.people.map((person) => ({
      person_id: person.person_id.trim(),
      display_name: person.display_name.trim(),
      aliases: person.aliases.map((alias) => alias.trim()),
      department: optional(person.department),
      role: optional(person.role),
      enabled: person.enabled,
    })),
    terms: profile.terms.map((term) => ({
      term_id: term.term_id.trim(),
      canonical: term.canonical.trim(),
      aliases: term.aliases.map((alias) => alias.trim()),
      category: optional(term.category),
      enabled: term.enabled,
    })),
  };
}

await call("Runtime.enable");
const before = await invoke("get_recording_state");
if (before.is_recording || before.is_active) {
  throw new Error("Refusing to start because Meetily is already recording");
}

const details = await invoke("api_get_template_v2", { request: { templateId } });
const template = details.template;
const profile = normalizeProfile(template.extensions.meetily_meeting_context);
const expectedProfileSha256 = sha256Json(profile);
const enabledPeople = profile.people.filter((person) => person.enabled);
const enabledTerms = profile.terms.filter((term) => term.enabled);
const attendance = enabledPeople.map((person) => ({
  personId: person.person_id,
  attendance: "attending",
}));
const meetingName = `${meetingNamePrefix}-${Date.now()}`;

await invoke("start_recording_with_devices_and_meeting", {
  micDeviceName: null,
  systemDeviceName: null,
  meetingName,
  templateSelection: {
    templateId: template.id,
    templateVersion: template.version,
    templateFileSha256: details.fileSha256,
  },
  meetingContextDraft: {
    expectedProfileSha256,
    attendance,
    hostPersonId: null,
    guests: [],
    additionalTerms: [],
  },
});

const deadline = performance.now() + 15000;
let after = null;
while (performance.now() < deadline) {
  after = await invoke("get_recording_state");
  if (after.is_recording && after.is_active) break;
  await new Promise((resolve) => setTimeout(resolve, 200));
}
const folder = await invoke("get_meeting_folder_path");
if (!after?.is_recording || !after?.is_active || !folder) {
  throw new Error(`Recording did not become active: ${JSON.stringify({ after, folder })}`);
}

const report = {
  startedAt: new Date().toISOString(),
  href: page.url,
  before,
  after,
  meetingName,
  folder,
  template: {
    id: template.id,
    name: template.name,
    version: template.version,
    fileSha256: details.fileSha256,
    expectedProfileSha256,
  },
  attendance,
  people: enabledPeople,
  terms: enabledTerms,
  hostPersonId: null,
  verdict: {
    backendStarted: after.is_recording === true && after.is_active === true,
    expectedTemplatePinned: template.id === templateId
      && template.version === expectedTemplateVersion,
    allSixteenUniquePeopleAttending: attendance.length === 16
      && new Set(attendance.map((item) => item.personId)).size === 16
      && attendance.every((item) => item.attendance === "attending"),
    eightTemplateTermsPinned: enabledTerms.length === 8,
    hostLeftUnspecified: true,
  },
};
fs.mkdirSync(path.dirname(outputPath), { recursive: true });
fs.writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
console.log(JSON.stringify(report, null, 2));
socket.close();
