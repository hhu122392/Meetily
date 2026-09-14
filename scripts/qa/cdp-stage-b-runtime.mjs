import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const outputPath = process.argv[2];
if (!outputPath) {
  throw new Error("Usage: node cdp-stage-b-runtime.mjs <report.json>");
}

const cdpPort = process.env.CDP_PORT ?? "9233";
const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`).then((response) =>
  response.json(),
);
const page = targets.find(
  (target) => target.type === "page" && (
    target.url.startsWith("http://tauri.localhost") ||
    target.url.startsWith("http://localhost:")
  ),
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
  return evaluate(`(async () => {
    try {
      return { ok: true, value: await window.__TAURI_INTERNALS__.invoke(
        ${JSON.stringify(command)},
        ${JSON.stringify(payload)}
      ) };
    } catch (error) {
      return { ok: false, error: typeof error === 'string' ? error : JSON.stringify(error) };
    }
  })()`);
}

function requireOk(result, label) {
  if (!result.ok) throw new Error(`${label} failed: ${result.error}`);
  return result.value;
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

function canonicalize(value) {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left < right ? -1 : left > right ? 1 : 0)
        .map(([key, item]) => [key, canonicalize(item)]),
    );
  }
  return value;
}

function sha256Json(value) {
  return crypto
    .createHash("sha256")
    .update(JSON.stringify(canonicalize(value)), "utf8")
    .digest("hex");
}

async function wait(milliseconds) {
  await new Promise((resolve) => setTimeout(resolve, milliseconds));
}

await call("Runtime.enable");

const startedAt = new Date().toISOString();
const uiProbe = await evaluate(`(async () => {
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const setupButton = [...document.querySelectorAll('button')]
    .find((element) => element.textContent?.includes('本次出席可调整'));
  if (!setupButton) return { ok: false, reason: 'setup button not found' };
  setupButton.click();
  await wait(250);
  const dialog = document.querySelector('[role="dialog"]');
  if (!dialog) return { ok: false, reason: 'setup dialog not found' };
  const combo = dialog.querySelector('[role="combobox"]');
  if (!combo) return { ok: false, reason: 'template selector not found' };
  combo.click();
  await wait(250);
  const monthlyOption = [...document.querySelectorAll('[role="option"]')]
    .find((element) => element.textContent?.includes('公司月会纪要'));
  if (!monthlyOption) return { ok: false, reason: 'monthly template option not found' };
  monthlyOption.click();
  await wait(650);
  const updatedDialog = document.querySelector('[role="dialog"]');
  const text = updatedDialog?.textContent ?? '';
  const comboboxCount = updatedDialog?.querySelectorAll('[role="combobox"]').length ?? 0;
  const doneButton = [...(updatedDialog?.querySelectorAll('button') ?? [])]
    .find((element) => element.textContent?.trim() === '完成');
  doneButton?.click();
  return {
    ok: true,
    selectedMonthlyTemplate: text.includes('公司月会纪要'),
    rosterRendered: text.includes('Mico') && text.includes('MeiL') && text.includes('Bono HU'),
    termsControlsRendered: text.includes('本次临时术语'),
    guestControlsRendered: text.includes('临时参会人'),
    comboboxCount,
  };
})()`);
if (!uiProbe.ok
  || !uiProbe.selectedMonthlyTemplate
  || !uiProbe.rosterRendered
  || !uiProbe.termsControlsRendered
  || !uiProbe.guestControlsRendered) {
  throw new Error(`Stage B UI probe failed: ${JSON.stringify(uiProbe)}`);
}

const list = requireOk(
  await invoke("api_list_templates_v2", { request: {} }),
  "list templates",
);
const summary = list.templates.find((template) => template.id === "company_monthly_review");
if (!summary || !summary.valid) throw new Error("Valid company monthly template was not found");
const details = requireOk(
  await invoke("api_get_template_v2", { request: { templateId: summary.id } }),
  "get company monthly template",
);
const profile = normalizeProfile(details.template.extensions.meetily_meeting_context);
const profileSha256 = sha256Json(profile);

const selection = {
  templateId: summary.id,
  templateVersion: summary.version,
  templateFileSha256: summary.fileSha256,
};
const staleResult = await invoke("start_recording_with_devices_and_meeting", {
  micDeviceName: null,
  systemDeviceName: null,
  meetingName: "QA-阶段B-陈旧模板拒绝",
  templateSelection: { ...selection, templateVersion: selection.templateVersion + 1 },
  meetingContextDraft: null,
});
if (staleResult.ok || !staleResult.error.includes("RECORDING_TEMPLATE_STALE")) {
  throw new Error(`Stale template was not rejected: ${JSON.stringify(staleResult)}`);
}
const recordingAfterStale = requireOk(await invoke("is_recording"), "recording state after stale test");
if (recordingAfterStale !== false) throw new Error("Stale selection unexpectedly started recording");

const host = profile.people.find((person) => person.person_id === "person_meil");
const absent = profile.people.find((person) => person.person_id === "person_mengniu");
if (!host || !absent) throw new Error("Expected monthly roster members were not found");
const attendance = profile.people.map((person) => ({
  personId: person.person_id,
  attendance: person.person_id === host.person_id
    ? "attending"
    : person.person_id === absent.person_id
      ? "absent"
      : "expected",
}));
const draft = {
  expectedProfileSha256: profileSha256,
  attendance,
  hostPersonId: host.person_id,
  guests: [{
    personId: "person_qa_guest_20260826",
    displayName: "QA测试嘉宾",
    aliases: ["测试嘉宾"],
    department: "质量保障",
    role: "验收人",
  }],
  additionalTerms: [{
    termId: "term_qa_acceptance_20260826",
    canonical: "阶段B验收",
    aliases: ["B阶段验收"],
    category: "测试术语",
  }],
};

const existingMetadataPath = process.env.STAGE_B_EXISTING_METADATA;
let meetingName;
let meetingFolder;
let metadataPath;
let initialContext;
let finalMetadata;
if (existingMetadataPath) {
  metadataPath = path.resolve(existingMetadataPath);
  meetingFolder = path.dirname(metadataPath);
  finalMetadata = JSON.parse(fs.readFileSync(metadataPath, "utf8"));
  meetingName = finalMetadata.meeting_name;
  initialContext = finalMetadata.meeting_context;
} else {
  meetingName = `QA-阶段B人员预设-${Date.now()}`;
  requireOk(await invoke("start_recording_with_devices_and_meeting", {
    micDeviceName: null,
    systemDeviceName: null,
    meetingName,
    templateSelection: selection,
    meetingContextDraft: draft,
  }), "start recording with meeting context");

  await wait(2_000);
  const active = requireOk(await invoke("is_recording"), "active recording state");
  if (active !== true) throw new Error("Recording did not stay active");
  meetingFolder = requireOk(await invoke("get_meeting_folder_path"), "meeting folder");
  if (!meetingFolder) throw new Error("Recording meeting folder was not created");
  metadataPath = path.join(meetingFolder, "metadata.json");
  const initialMetadata = JSON.parse(fs.readFileSync(metadataPath, "utf8"));
  initialContext = initialMetadata.meeting_context;
  if (!initialContext) throw new Error("Initial metadata did not contain meeting_context");

  await wait(3_000);
  requireOk(await invoke("stop_recording", {
    args: { save_path: path.join(meetingFolder, "recording.wav") },
  }), "stop recording");
  const completed = requireOk(await invoke("is_recording"), "completed recording state");
  if (completed !== false) throw new Error("Recording remained active after stop");
  finalMetadata = JSON.parse(fs.readFileSync(metadataPath, "utf8"));
}
if (!initialContext) throw new Error("Metadata did not contain meeting_context");
const finalContext = finalMetadata.meeting_context;
const recordingSnapshot = finalContext?.contexts?.find((context) => (
  context.context_id === finalContext.recording_context_id
));

const assertions = {
  templatePinned: finalMetadata.summary_template?.template_id === summary.id
    && finalMetadata.summary_template?.template_version === summary.version
    && finalMetadata.summary_template?.template_file_sha256 === summary.fileSha256,
  contextIdentityPreserved: Boolean(
    initialContext.recording_context_id
    && initialContext.recording_context_id === finalContext?.recording_context_id,
  ),
  contextHashPreserved: Boolean(
    initialContext.contexts?.[0]?.context_sha256
    && initialContext.contexts[0].context_sha256 === recordingSnapshot?.context_sha256,
  ),
  rosterAndTermsStored: recordingSnapshot?.people?.length === 26
    && recordingSnapshot?.terms?.length === 6,
  attendanceStored: recordingSnapshot?.people?.find((person) => person.person_id === host.person_id)?.attendance === "attending"
    && recordingSnapshot?.people?.find((person) => person.person_id === absent.person_id)?.attendance === "absent",
  hostStored: recordingSnapshot?.host_person_id === host.person_id,
  guestStored: recordingSnapshot?.people?.some((person) => (
    person.person_id === "person_qa_guest_20260826"
    && person.display_name === "QA测试嘉宾"
    && person.attendance === "guest"
  )),
  additionalTermStored: recordingSnapshot?.terms?.some((term) => (
    term.term_id === "term_qa_acceptance_20260826"
    && term.canonical === "阶段B验收"
  )),
  metadataCompleted: finalMetadata.status === "completed" && Boolean(finalMetadata.completed_at),
};
const failedAssertions = Object.entries(assertions)
  .filter(([, passed]) => !passed)
  .map(([name]) => name);
if (failedAssertions.length > 0) {
  throw new Error(`Stage B metadata assertions failed: ${failedAssertions.join(", ")}`);
}

const report = {
  stage: "B",
  startedAt,
  finishedAt: new Date().toISOString(),
  pageUrl: page.url,
  uiProbe,
  template: {
    id: summary.id,
    version: summary.version,
    fileSha256: summary.fileSha256,
    profileSha256,
    fixedPeople: profile.people.length,
    fixedTerms: profile.terms.length,
  },
  staleSelectionRejected: true,
  realRecording: {
    meetingName,
    meetingFolder,
    metadataPath,
    reusedCompletedRecording: Boolean(existingMetadataPath),
    recordingContextId: finalContext.recording_context_id,
    contextSha256: recordingSnapshot.context_sha256,
    status: finalMetadata.status,
  },
  assertions,
  result: "PASS",
};
fs.mkdirSync(path.dirname(outputPath), { recursive: true });
fs.writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
console.log(JSON.stringify(report, null, 2));
socket.close();
