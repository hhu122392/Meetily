#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

const root = path.resolve(process.argv[2] || ".");
const reportPath = path.join(root, "docs/i18n/audit/phase-3-native/static-audit.json");
const findings = [];
const checks = [];

const read = (relativePath) => fs.readFile(path.join(root, relativePath), "utf8");
const check = (id, passed, detail) => {
  checks.push({ id, passed, detail });
  if (!passed) findings.push({ id, detail });
};

const en = JSON.parse(await read("frontend/src-tauri/src/i18n/locales/en.json"));
const zh = JSON.parse(await read("frontend/src-tauri/src/i18n/locales/zh-CN.json"));
const enKeys = Object.keys(en).sort();
const zhKeys = Object.keys(zh).sort();
check("locale-key-parity", JSON.stringify(enKeys) === JSON.stringify(zhKeys), {
  en: enKeys.length,
  zhCN: zhKeys.length,
});
check(
  "locale-values-non-empty",
  [...Object.values(en), ...Object.values(zh)].every(
    (value) => typeof value === "string" && value.trim(),
  ),
  { entries: enKeys.length * 2 },
);

const placeholders = (value) =>
  [...String(value).matchAll(/{{\s*([^}\s]+)\s*}}/g)].map((match) => match[1]).sort();
const placeholderMismatches = enKeys.filter(
  (key) => JSON.stringify(placeholders(en[key])) !== JSON.stringify(placeholders(zh[key])),
);
check("placeholder-parity", placeholderMismatches.length === 0, placeholderMismatches);

const requiredKeys = [
  "tray.downloadingTranscriptionModel",
  "tray.startRecording",
  "tray.startingRecording",
  "tray.pauseRecording",
  "tray.stopRecording",
  "tray.pausing",
  "tray.resumeRecording",
  "tray.resuming",
  "tray.stopping",
  "tray.openMainWindow",
  "tray.settings",
  "tray.checkForUpdates",
  "tray.quit",
  "notification.recordingStarted",
  "notification.recordingStopped",
  "notification.recordingPaused",
  "notification.recordingResumed",
  "notification.transcriptionComplete",
  "notification.meetingReminder",
  "notification.systemErrorBody",
  "notification.test",
];
check(
  "required-native-keys",
  requiredKeys.every((key) => key in en && key in zh),
  requiredKeys.filter((key) => !(key in en) || !(key in zh)),
);

const tray = await read("frontend/src-tauri/src/tray.rs");
const trayIds = [
  "toggle_recording",
  "pause_recording",
  "resume_recording",
  "stop_recording",
  "open_window",
  "settings",
  "check_updates",
  "quit",
];
check(
  "stable-tray-action-ids",
  trayIds.every((id) => tray.includes(`\"${id}\"`)),
  trayIds.filter((id) => !tray.includes(`\"${id}\"`)),
);
const legacyTrayLabels = [
  "Downloading transcription model...",
  "Start Recording",
  "Starting Recording...",
  "Pause Recording",
  "Stop Recording",
  "Pausing...",
  "Resume Recording",
  "Resuming...",
  "Stopping...",
  "Open Main Window",
  "Check for Updates",
  'with_id("quit", "Quit")',
];
check(
  "no-hardcoded-tray-labels",
  legacyTrayLabels.every((label) => !tray.includes(label)),
  legacyTrayLabels.filter((label) => tray.includes(label)),
);
check(
  "tray-hot-rebuild",
  tray.includes("refresh_tray_for_locale") && tray.includes("update_tray_menu_async"),
  "Locale refresh must rebuild the existing tray from current recording state.",
);

const lib = await read("frontend/src-tauri/src/lib.rs");
const audioMod = await read("frontend/src-tauri/src/audio/mod.rs");
check(
  "native-locale-command-registration",
  lib.includes("i18n::get_ui_locale") &&
    lib.includes("i18n::set_ui_locale") &&
    lib.includes("NativeI18nState::load"),
  "get/set commands and setup state must all be registered.",
);
const oldTokens = ["lib_old_complex", "core-old", "recording_saver_old"];
check(
  "legacy-source-not-compiled",
  oldTokens.every((token) => !lib.includes(token) && !audioMod.includes(token)),
  oldTokens.filter((token) => lib.includes(token) || audioMod.includes(token)),
);

const types = await read("frontend/src-tauri/src/notifications/types.rs");
const commands = await read("frontend/src-tauri/src/notifications/commands.rs");
const manager = await read("frontend/src-tauri/src/notifications/manager.rs");
check(
  "notification-copy-localized",
  [types, commands, manager].join("\n").includes("notification.recordingStarted") &&
    [types, commands, manager].join("\n").includes("notification.recordingStopped") &&
    !types.includes("Transcription completed and saved to:"),
  "Notification constructors and fallbacks must use native locale keys.",
);
check(
  "system-error-notification-redacted",
  types.includes("NotificationType::SystemError") &&
    !types.includes("NotificationType::SystemError(error_string)") &&
    manager.includes("Preserve the raw diagnostic in logs only"),
  "Raw system errors must not be rendered by OS notifications.",
);
check(
  "notification-command-errors-structured",
  !/Result<[^>]*,\s*String>/.test(commands) &&
    commands.includes('NativeError::new("NOTIFICATION_MANAGER_UNAVAILABLE")'),
  "All exported notification command failures must serialize NativeError.",
);

const nativeI18n = await read("frontend/src-tauri/src/i18n/mod.rs");
const nativeErrors = await read("frontend/src-tauri/src/i18n/error.rs");
const frontendBridge = await read("frontend/src/lib/native-i18n.ts");
const provider = await read("frontend/src/i18n/I18nProvider.tsx");
const registryCodes = [
  ...nativeErrors.matchAll(/code:\s*"([A-Z0-9_]+)"/g),
].map((match) => match[1]);
check(
  "error-registry-unique",
  registryCodes.length > 0 && new Set(registryCodes).size === registryCodes.length,
  { codes: registryCodes },
);
check(
  "unknown-error-safe-fallback",
  nativeErrors.includes('registration("NATIVE_UNKNOWN")') &&
    frontendBridge.includes('?? "common:errors.unknown"'),
  "Both Rust and React boundaries must have a safe unknown fallback.",
);
check(
  "locale-persistence",
  nativeI18n.includes("ui-locale.json") &&
    nativeI18n.includes("replace_and_persist") &&
    nativeI18n.includes("fs::write"),
  "Resolved locale and preference must survive restart.",
);
check(
  "react-native-locale-sync",
  provider.match(/syncNativeUiLocale/g)?.length === 3 &&
    frontendBridge.includes('invoke<NativeUiLocaleSnapshot>("set_ui_locale"'),
  "Provider import plus initial/change synchronization and IPC invoke are required.",
);

const matrix = JSON.parse(await read("docs/i18n/phase-3/native-candidate-disposition.json"));
check(
  "candidate-matrix-complete",
  matrix.summary.total === 492 &&
    matrix.summary.active === 447 &&
    matrix.summary.excludedLegacy === 45 &&
    matrix.summary.unresolved === 0,
  matrix.summary,
);

const report = {
  phase: "15.6-stage-3-tauri-native-i18n",
  generatedAt: new Date().toISOString(),
  passed: findings.length === 0,
  summary: { checks: checks.length, passed: checks.filter((item) => item.passed).length, failed: findings.length },
  checks,
  findings,
};
await fs.mkdir(path.dirname(reportPath), { recursive: true });
await fs.writeFile(reportPath, JSON.stringify(report, null, 2) + "\n");
process.stdout.write(JSON.stringify({ reportPath, ...report.summary, passed: report.passed }) + "\n");
if (!report.passed) process.exitCode = 1;
