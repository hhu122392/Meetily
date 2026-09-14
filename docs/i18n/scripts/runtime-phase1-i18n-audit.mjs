#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

const port = Number(process.argv[2] || 9331);
const outputDirectory = path.resolve(
  process.argv[3] || "docs/i18n/audit/phase-1-runtime/cdp",
);
const mode = process.argv[4] || "switch";

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
      this.socket.addEventListener("open", resolve, { once: true });
      this.socket.addEventListener("error", reject, { once: true });
    });
    this.socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data);
      if (!message.id) {
        this.events.push(message);
        return;
      }
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id);
      if (message.error) pending.reject(new Error(JSON.stringify(message.error)));
      else pending.resolve(message.result);
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

const sleep = (milliseconds) =>
  new Promise((resolve) => setTimeout(resolve, milliseconds));

async function findTarget() {
  const deadline = Date.now() + 20_000;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json`);
      if (response.ok) {
        const targets = await response.json();
        const target = targets.find((candidate) => candidate.type === "page");
        if (target?.webSocketDebuggerUrl) return target;
      }
    } catch (error) {
      lastError = error;
    }
    await sleep(100);
  }
  throw new Error(`No CDP page target on port ${port}: ${String(lastError || "timeout")}`);
}

async function evaluate(client, expression) {
  const response = await client.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (response.exceptionDetails) {
    throw new Error(
      response.exceptionDetails.exception?.description ||
        response.exceptionDetails.text ||
        "Runtime evaluation failed",
    );
  }
  return response.result.value;
}

async function waitFor(client, expression, description, timeout = 15_000) {
  const deadline = Date.now() + timeout;
  let value;
  while (Date.now() < deadline) {
    value = await evaluate(client, expression);
    if (value) return value;
    await sleep(100);
  }
  throw new Error(`Timed out waiting for ${description}; last=${JSON.stringify(value)}`);
}

async function waitForDocument(client) {
  await waitFor(
    client,
    `document.readyState === "complete" && Boolean(document.querySelector("[data-i18n-ready='true']"))`,
    "document and i18n provider readiness",
  );
  await sleep(350);
}

async function press(client, key, code, virtualKeyCode) {
  await client.send("Input.dispatchKeyEvent", {
    type: "keyDown",
    key,
    code,
    windowsVirtualKeyCode: virtualKeyCode,
  });
  await client.send("Input.dispatchKeyEvent", {
    type: "keyUp",
    key,
    code,
    windowsVirtualKeyCode: virtualKeyCode,
  });
}

async function snapshot(client, name) {
  const data = await evaluate(
    client,
    `(() => {
      const select = document.querySelector("select[aria-label='Display language'], select[aria-label='显示语言']");
      const gate = document.querySelector("[data-i18n-ready]");
      return {
        name: ${JSON.stringify(name)},
        capturedAt: new Date().toISOString(),
        url: location.href,
        readyState: document.readyState,
        htmlLang: document.documentElement.lang,
        htmlDir: document.documentElement.dir,
        gateReady: gate?.getAttribute("data-i18n-ready") || null,
        gateVisibility: gate ? getComputedStyle(gate).visibility : null,
        bodyText: document.body?.innerText || "",
        select: select ? {
          ariaLabel: select.getAttribute("aria-label"),
          value: select.value,
          options: [...select.options].map((option) => ({ value: option.value, text: option.text })),
        } : null,
        bootstrap: window.__MEETILY_UI_LOCALE_BOOTSTRAP__ || null,
        storage: {
          uiLocale: localStorage.getItem("meetily.uiLocale"),
          primaryLanguage: localStorage.getItem("primaryLanguage"),
          summaryLanguage: localStorage.getItem("summaryLanguage"),
          summaryLanguageDefault: localStorage.getItem("summaryLanguageDefault"),
          providerModelMap: localStorage.getItem("providerModelMap"),
          downloadingModels: localStorage.getItem("downloading-models"),
        },
        inputs: [...document.querySelectorAll("input, textarea, [contenteditable='true']")].map((element) => ({
          placeholder: element.getAttribute("placeholder"),
          value: element.value ?? element.textContent ?? "",
        })),
        resources: performance.getEntriesByType("resource").map((entry) => entry.name),
      };
    })()`,
  );
  const image = await client.send("Page.captureScreenshot", {
    format: "png",
    captureBeyondViewport: false,
  });
  await Promise.all([
    fs.writeFile(path.join(outputDirectory, `${name}.json`), `${JSON.stringify(data, null, 2)}\n`),
    fs.writeFile(path.join(outputDirectory, `${name}.png`), Buffer.from(image.data, "base64")),
  ]);
  return data;
}

function diagnosticSummary(events) {
  const diagnostics = events.filter((event) =>
    ["Runtime.exceptionThrown", "Runtime.consoleAPICalled", "Log.entryAdded"].includes(
      event.method,
    ),
  );
  const runtimeExceptions = diagnostics.filter(
    (event) => event.method === "Runtime.exceptionThrown",
  );
  const consoleErrors = diagnostics.filter(
    (event) =>
      event.method === "Runtime.consoleAPICalled" && event.params?.type === "error",
  );
  const logErrors = diagnostics.filter(
    (event) =>
      event.method === "Log.entryAdded" &&
      ["error", "warning"].includes(event.params?.entry?.level),
  );
  const cspIpcEntries = diagnostics.filter((event) => {
    const encoded = JSON.stringify(event.params || {});
    return encoded.includes("ipc.localhost") && /content security policy|csp/i.test(encoded);
  });
  const expectedEnvironmentConsoleErrors = consoleErrors.filter((event) => {
    const encoded = JSON.stringify(event.params || {});
    return encoded.includes("Ollama server is running") && encoded.includes("Request timed out");
  });
  const unexpectedConsoleErrors = consoleErrors.filter(
    (event) => !expectedEnvironmentConsoleErrors.includes(event),
  );
  return {
    diagnostics,
    runtimeExceptions: runtimeExceptions.length,
    consoleErrors: consoleErrors.length,
    expectedEnvironmentConsoleErrors: expectedEnvironmentConsoleErrors.length,
    unexpectedConsoleErrors: unexpectedConsoleErrors.length,
    logErrors: logErrors.length,
    cspIpcEntries: cspIpcEntries.length,
  };
}

async function installBootTrace(client) {
  await client.send("Page.addScriptToEvaluateOnNewDocument", {
    source: `(() => {
      window.__phase1BootTrace = [];
      let queued = false;
      const capture = (reason) => {
        queued = false;
        const gate = document.querySelector?.("[data-i18n-ready]");
        window.__phase1BootTrace.push({
          reason,
          now: performance.now(),
          readyState: document.readyState,
          htmlLang: document.documentElement?.lang || null,
          htmlDir: document.documentElement?.dir || null,
          gateReady: gate?.getAttribute("data-i18n-ready") || null,
          gateVisibility: gate ? getComputedStyle(gate).visibility : null,
          bodyText: (document.body?.innerText || "").replace(/\\s+/g, " ").trim().slice(0, 500),
        });
      };
      const queueCapture = () => {
        if (queued) return;
        queued = true;
        queueMicrotask(() => capture("mutation"));
      };
      new MutationObserver(queueCapture).observe(document, {
        subtree: true,
        childList: true,
        characterData: true,
        attributes: true,
        attributeFilter: ["lang", "dir", "style", "data-i18n-ready"],
      });
      capture("installed");
      requestAnimationFrame(() => capture("raf-1"));
      requestAnimationFrame(() => requestAnimationFrame(() => capture("raf-2")));
    })();`,
  });
}

async function ensureCompletedOnboarding(client) {
  return evaluate(
    client,
    `(async () => {
      const status = {
        version: "1.0",
        completed: true,
        current_step: 4,
        model_status: {
          parakeet: "not_downloaded",
          summary: "not_downloaded",
          selected_summary_model: "Qwen3.5-2B-Q4_K_M.gguf",
        },
        last_updated: new Date().toISOString(),
      };
      await window.__TAURI_INTERNALS__.invoke("save_onboarding_status_cmd", { status });
      return window.__TAURI_INTERNALS__.invoke("get_onboarding_status");
    })()`,
  );
}

async function navigateToSettings(client) {
  await client.send("Page.navigate", { url: "http://tauri.localhost/settings" });
  await waitForDocument(client);
  await waitFor(
    client,
    `Boolean(document.querySelector("select[aria-label='Display language'], select[aria-label='显示语言']"))`,
    "display-language select",
  );
}

async function runSwitchAudit(client, target) {
  await waitForDocument(client);
  const onboarding = await ensureCompletedOnboarding(client);
  await navigateToSettings(client);

  await evaluate(
    client,
    `(() => {
      const before = localStorage.getItem("meetily.uiLocale");
      localStorage.setItem("primaryLanguage", "ja");
      localStorage.setItem("summaryLanguage", "de");
      localStorage.setItem("summaryLanguageDefault", "fr");
      localStorage.setItem("providerModelMap", JSON.stringify({ phase1Audit: "sentinel-model" }));
      localStorage.setItem("downloading-models", JSON.stringify(["phase1-synthetic-in-progress"]));
      localStorage.setItem("meetily.uiLocale", "en");
      dispatchEvent(new StorageEvent("storage", {
        key: "meetily.uiLocale",
        oldValue: before,
        newValue: "en",
        storageArea: localStorage,
      }));
      return true;
    })()`,
  );
  await waitFor(
    client,
    `document.documentElement.lang === "en" && document.querySelector("select[aria-label='Display language']")?.value === "en"`,
    "English locale activation",
  );
  const english = await snapshot(client, "01-settings-english");

  const focused = await evaluate(
    client,
    `(() => {
      const select = document.querySelector("select[aria-label='Display language']");
      select?.focus();
      return document.activeElement === select;
    })()`,
  );
  if (!focused) throw new Error("Unable to focus display-language select");
  await press(client, "End", "End", 35);
  await waitFor(
    client,
    `document.documentElement.lang === "zh-CN" && document.querySelector("select[aria-label='显示语言']")?.value === "zh-CN" && localStorage.getItem("meetily.uiLocale") === "zh-CN"`,
    "Simplified Chinese locale activation through keyboard input",
  );
  const chinese = await snapshot(client, "02-settings-chinese-after-keyboard");

  const chineseSelectFocused = await evaluate(
    client,
    `(() => {
      const select = document.querySelector("select[aria-label='显示语言']");
      select?.focus();
      return document.activeElement === select;
    })()`,
  );
  if (!chineseSelectFocused) throw new Error("Unable to refocus Chinese display-language select");
  await press(client, "ArrowUp", "ArrowUp", 38);
  await waitFor(
    client,
    `document.documentElement.lang === "en" && document.querySelector("select[aria-label='Display language']")?.value === "en"`,
    "English reactivation through keyboard input",
  );
  const englishAgain = await snapshot(client, "03-settings-english-again");

  const englishSelectFocused = await evaluate(
    client,
    `(() => {
      const select = document.querySelector("select[aria-label='Display language']");
      select?.focus();
      return document.activeElement === select;
    })()`,
  );
  if (!englishSelectFocused) throw new Error("Unable to refocus English display-language select");
  await press(client, "End", "End", 35);
  await waitFor(
    client,
    `document.documentElement.lang === "zh-CN" && document.querySelector("select[aria-label='显示语言']")?.value === "zh-CN"`,
    "Chinese restoration before persistence checks",
  );

  await client.send("Network.emulateNetworkConditions", {
    offline: true,
    latency: 0,
    downloadThroughput: 0,
    uploadThroughput: 0,
  });
  await client.send("Page.reload", { ignoreCache: true });
  let offlineReloadError = null;
  try {
    await waitForDocument(client);
    await waitFor(
      client,
      `document.documentElement.lang === "zh-CN" && document.body.innerText.includes("显示语言")`,
      "offline Chinese settings reload",
      12_000,
    );
  } catch (error) {
    offlineReloadError = String(error);
  } finally {
    await client.send("Network.emulateNetworkConditions", {
      offline: false,
      latency: 0,
      downloadThroughput: -1,
      uploadThroughput: -1,
    });
  }
  const offline = await snapshot(client, "04-settings-chinese-offline-reload");

  const recordingBeforeEdit = await evaluate(
    client,
    `window.__TAURI_INTERNALS__.invoke("is_recording")`,
  );
  await client.send("Page.navigate", { url: "http://tauri.localhost/" });
  await waitForDocument(client);
  const sidebarToggle = await evaluate(
    client,
    `(() => {
      const button = [...document.querySelectorAll("button")].find((candidate) => {
        const rect = candidate.getBoundingClientRect();
        return rect.left >= 50 && rect.left < 120 && rect.top >= 50 && rect.top < 130;
      });
      if (!button) return null;
      const rect = button.getBoundingClientRect();
      return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    })()`,
  );
  if (!sidebarToggle) throw new Error("Unable to locate sidebar expansion control");
  await client.send("Input.dispatchMouseEvent", {
    type: "mousePressed",
    x: sidebarToggle.x,
    y: sidebarToggle.y,
    button: "left",
    clickCount: 1,
  });
  await client.send("Input.dispatchMouseEvent", {
    type: "mouseReleased",
    x: sidebarToggle.x,
    y: sidebarToggle.y,
    button: "left",
    clickCount: 1,
  });
  await waitFor(
    client,
    `Boolean(document.querySelector("input[placeholder='Search meeting content...']"))`,
    "main-page meeting search input",
  );
  const searchFocused = await evaluate(
    client,
    `(() => {
      const input = document.querySelector("input[placeholder='Search meeting content...']");
      input?.focus();
      return document.activeElement === input;
    })()`,
  );
  if (!searchFocused) throw new Error("Unable to focus meeting search input");
  await client.send("Input.insertText", { text: "phase1-edit-sentinel" });
  await waitFor(
    client,
    `document.querySelector("input[placeholder='Search meeting content...']")?.value === "phase1-edit-sentinel"`,
    "controlled search input edit",
  );
  await evaluate(
    client,
    `(() => {
      const before = localStorage.getItem("meetily.uiLocale");
      localStorage.setItem("meetily.uiLocale", "en");
      dispatchEvent(new StorageEvent("storage", {
        key: "meetily.uiLocale",
        oldValue: before,
        newValue: "en",
        storageArea: localStorage,
      }));
      return true;
    })()`,
  );
  await waitFor(
    client,
    `document.documentElement.lang === "en" && document.querySelector("input[placeholder='Search meeting content...']")?.value === "phase1-edit-sentinel"`,
    "English switch while controlled input is edited",
  );
  const editing = await snapshot(client, "05-main-edit-preserved-after-switch");
  await evaluate(
    client,
    `(() => {
      const before = localStorage.getItem("meetily.uiLocale");
      localStorage.setItem("meetily.uiLocale", "zh-CN");
      dispatchEvent(new StorageEvent("storage", {
        key: "meetily.uiLocale",
        oldValue: before,
        newValue: "zh-CN",
        storageArea: localStorage,
      }));
      return true;
    })()`,
  );
  await waitFor(client, `document.documentElement.lang === "zh-CN"`, "Chinese restoration after edit audit");
  const recordingAfterEdit = await evaluate(
    client,
    `window.__TAURI_INTERNALS__.invoke("is_recording")`,
  );

  const sameState = (candidate) => {
    let modelMap = {};
    try {
      modelMap = JSON.parse(candidate.storage.providerModelMap || "{}");
    } catch {
      return false;
    }
    return (
      candidate.storage.primaryLanguage === "ja" &&
      candidate.storage.summaryLanguage === "de" &&
      candidate.storage.summaryLanguageDefault === "fr" &&
      modelMap.phase1Audit === "sentinel-model" &&
      candidate.storage.downloadingModels === '["phase1-synthetic-in-progress"]'
    );
  };
  const externalResources = offline.resources.filter((resource) => {
    try {
      return !["tauri.localhost", "ipc.localhost"].includes(new URL(resource).hostname);
    } catch {
      return false;
    }
  });

  const report = {
    scope: "PHASE_1_STRICT_I18N_RUNTIME_SWITCH_AUDIT",
    startedAt: new Date().toISOString(),
    target: { id: target.id, title: target.title, url: target.url },
    onboarding,
    checks: [
      {
        id: "P1-RUN-SELECT-KEYBOARD",
        pass:
          english.select?.value === "en" &&
          chinese.select?.value === "zh-CN" &&
          englishAgain.select?.value === "en",
        evidence: [
          "01-settings-english.json",
          "02-settings-chinese-after-keyboard.json",
          "03-settings-english-again.json",
        ],
      },
      {
        id: "P1-RUN-HTML-METADATA",
        pass:
          english.htmlLang === "en" &&
          english.htmlDir === "ltr" &&
          chinese.htmlLang === "zh-CN" &&
          chinese.htmlDir === "ltr",
        evidence: "02-settings-chinese-after-keyboard.json",
      },
      {
        id: "P1-RUN-TRANSLATION-AND-FALLBACK",
        pass:
          chinese.bodyText.includes("显示语言") &&
          chinese.bodyText.includes("Data Storage Locations") &&
          !chinese.bodyText.includes("settings:labels.dataStorageLocations"),
        evidence: "02-settings-chinese-after-keyboard.json",
      },
      {
        id: "P1-RUN-LANGUAGE-AND-MODEL-STATE-ISOLATION",
        pass: sameState(chinese) && sameState(englishAgain) && sameState(offline) && sameState(editing),
        evidence: [
          "02-settings-chinese-after-keyboard.json",
          "03-settings-english-again.json",
          "04-settings-chinese-offline-reload.json",
          "05-main-edit-preserved-after-switch.json",
        ],
      },
      {
        id: "P1-RUN-MODEL-DOWNLOAD-IN-PROGRESS",
        pass: [chinese, englishAgain, offline].every(
          (item) =>
            /Summary Model[\s\S]*?\d+%/.test(item.bodyText),
        ),
        evidence: [
          "02-settings-chinese-after-keyboard.json",
          "03-settings-english-again.json",
          "04-settings-chinese-offline-reload.json",
        ],
      },
      {
        id: "P1-RUN-EDIT-IN-PROGRESS-PRESERVED",
        pass:
          editing.htmlLang === "en" &&
          new URL(editing.url).pathname === "/" &&
          editing.inputs.some((input) => input.value === "phase1-edit-sentinel"),
        evidence: "05-main-edit-preserved-after-switch.json",
      },
      {
        id: "P1-RUN-PRE-RECORDING-STATE-PRESERVED",
        pass: recordingBeforeEdit === false && recordingAfterEdit === false,
        evidence: "runtime-switch-report.json",
        recordingBeforeEdit,
        recordingAfterEdit,
      },
      {
        id: "P1-RUN-OFFLINE-BUNDLED-RESOURCES",
        pass:
          offlineReloadError === null &&
          offline.htmlLang === "zh-CN" &&
          offline.bodyText.includes("显示语言") &&
          externalResources.length === 0,
        evidence: "04-settings-chinese-offline-reload.json",
        offlineReloadError,
        externalResources,
      },
      {
        id: "P1-RUN-ROUTE-AND-GATE-STABILITY",
        pass:
          [english, chinese, englishAgain, offline].every(
            (item) =>
              new URL(item.url).pathname === "/settings" &&
              item.gateReady === "true" &&
              item.gateVisibility === "visible",
          ),
        evidence: "04-settings-chinese-offline-reload.json",
      },
    ],
  };
  const diagnostics = diagnosticSummary(client.events);
  await fs.writeFile(
    path.join(outputDirectory, "06-runtime-diagnostics.json"),
    `${JSON.stringify(diagnostics.diagnostics, null, 2)}\n`,
  );
  report.checks.push({
    id: "P1-RUN-NO-RUNTIME-CSP-ERRORS",
    pass:
      diagnostics.runtimeExceptions === 0 &&
      diagnostics.unexpectedConsoleErrors === 0 &&
      diagnostics.logErrors === 0 &&
      diagnostics.cspIpcEntries === 0,
    evidence: "06-runtime-diagnostics.json",
    counts: {
      runtimeExceptions: diagnostics.runtimeExceptions,
      consoleErrors: diagnostics.consoleErrors,
      expectedEnvironmentConsoleErrors: diagnostics.expectedEnvironmentConsoleErrors,
      unexpectedConsoleErrors: diagnostics.unexpectedConsoleErrors,
      logErrors: diagnostics.logErrors,
      cspIpcEntries: diagnostics.cspIpcEntries,
    },
  });
  report.nonBlockingObservations = [
    {
      id: "P1-ENV-OLLAMA-UNAVAILABLE",
      severity: "P3",
      count: diagnostics.expectedEnvironmentConsoleErrors,
      description:
        "Settings model discovery timed out because the optional local Ollama service is not running; this is classified separately from i18n/runtime/CSP errors.",
    },
  ];
  report.finishedAt = new Date().toISOString();
  report.summary = {
    passed: report.checks.filter((check) => check.pass).length,
    failed: report.checks.filter((check) => !check.pass).length,
    total: report.checks.length,
  };
  await fs.writeFile(
    path.join(outputDirectory, "runtime-switch-report.json"),
    `${JSON.stringify(report, null, 2)}\n`,
  );
  return report;
}

async function runColdAudit(client, target) {
  await waitForDocument(client);
  const initial = await snapshot(client, "01-cold-process-initial");
  await installBootTrace(client);
  await navigateToSettings(client);
  const trace = await evaluate(client, "window.__phase1BootTrace || []");
  const settings = await snapshot(client, "02-cold-settings");
  const visibleFrames = trace.filter(
    (entry) => entry.gateVisibility === "visible" && entry.bodyText,
  );
  const invalidVisibleFrames = visibleFrames.filter(
    (entry) => entry.htmlLang !== "zh-CN" || entry.gateReady !== "true",
  );
  const report = {
    scope: "PHASE_1_STRICT_I18N_COLD_START_AUDIT",
    startedAt: new Date().toISOString(),
    target: { id: target.id, title: target.title, url: target.url },
    checks: [
      {
        id: "P1-COLD-PERSISTED-PREFERENCE",
        pass:
          initial.storage.uiLocale === "zh-CN" &&
          initial.bootstrap?.preference === "zh-CN" &&
          initial.bootstrap?.locale === "zh-CN" &&
          initial.htmlLang === "zh-CN" &&
          initial.htmlDir === "ltr",
        evidence: "01-cold-process-initial.json",
      },
      {
        id: "P1-COLD-SETTINGS-SELECTION",
        pass:
          settings.select?.value === "zh-CN" &&
          settings.select?.ariaLabel === "显示语言" &&
          settings.bodyText.includes("显示语言"),
        evidence: "02-cold-settings.json",
      },
      {
        id: "P1-COLD-NO-WRONG-LANGUAGE-VISIBLE-GATE",
        pass: visibleFrames.length > 0 && invalidVisibleFrames.length === 0,
        evidence: "03-cold-boot-trace.json",
        visibleFrameCount: visibleFrames.length,
        invalidVisibleFrames,
      },
    ],
  };
  await fs.writeFile(
    path.join(outputDirectory, "03-cold-boot-trace.json"),
    `${JSON.stringify(trace, null, 2)}\n`,
  );
  const diagnostics = diagnosticSummary(client.events);
  await fs.writeFile(
    path.join(outputDirectory, "04-cold-diagnostics.json"),
    `${JSON.stringify(diagnostics.diagnostics, null, 2)}\n`,
  );
  report.checks.push({
    id: "P1-COLD-NO-RUNTIME-CSP-ERRORS",
    pass:
      diagnostics.runtimeExceptions === 0 &&
      diagnostics.consoleErrors === 0 &&
      diagnostics.logErrors === 0 &&
      diagnostics.cspIpcEntries === 0,
    evidence: "04-cold-diagnostics.json",
    counts: {
      runtimeExceptions: diagnostics.runtimeExceptions,
      consoleErrors: diagnostics.consoleErrors,
      logErrors: diagnostics.logErrors,
      cspIpcEntries: diagnostics.cspIpcEntries,
    },
  });
  report.finishedAt = new Date().toISOString();
  report.summary = {
    passed: report.checks.filter((check) => check.pass).length,
    failed: report.checks.filter((check) => !check.pass).length,
    total: report.checks.length,
  };
  await fs.writeFile(
    path.join(outputDirectory, "runtime-cold-report.json"),
    `${JSON.stringify(report, null, 2)}\n`,
  );
  return report;
}

async function runDevAudit(client, target) {
  await installBootTrace(client);
  await client.send("Page.reload", { ignoreCache: true });
  await waitForDocument(client);
  const page = await snapshot(client, "01-dev-page-after-reload");
  const trace = await evaluate(client, "window.__phase1BootTrace || []");
  const ipc = await evaluate(
    client,
    `(async () => ({
      available: typeof window.__TAURI_INTERNALS__?.invoke === "function",
      isRecording: await window.__TAURI_INTERNALS__.invoke("is_recording"),
    }))()`,
  );
  const visibleFrames = trace.filter(
    (entry) => entry.gateVisibility === "visible" && entry.bodyText,
  );
  const diagnostics = diagnosticSummary(client.events);
  await Promise.all([
    fs.writeFile(
      path.join(outputDirectory, "02-dev-boot-trace.json"),
      `${JSON.stringify(trace, null, 2)}\n`,
    ),
    fs.writeFile(
      path.join(outputDirectory, "03-dev-diagnostics.json"),
      `${JSON.stringify(diagnostics.diagnostics, null, 2)}\n`,
    ),
  ]);
  const report = {
    scope: "PHASE_1_STRICT_TAURI_DEV_RUNTIME_AUDIT",
    startedAt: new Date().toISOString(),
    target: { id: target.id, title: target.title, url: target.url },
    checks: [
      {
        id: "P1-DEV-PAGE-AND-I18N-GATE",
        pass:
          new URL(page.url).origin === "http://localhost:3118" &&
          page.htmlLang === "zh-CN" &&
          page.htmlDir === "ltr" &&
          page.gateReady === "true" &&
          page.gateVisibility === "visible" &&
          page.bodyText.includes("Welcome to meetily!"),
        evidence: "01-dev-page-after-reload.json",
      },
      {
        id: "P1-DEV-PERSISTED-BOOTSTRAP",
        pass:
          page.bootstrap?.preference === (page.storage.uiLocale || "system") &&
          page.bootstrap?.locale === page.htmlLang &&
          visibleFrames.length > 0 &&
          visibleFrames.every(
            (entry) => entry.htmlLang === "zh-CN" && entry.gateReady === "true",
          ),
        evidence: "02-dev-boot-trace.json",
      },
      {
        id: "P1-DEV-TAURI-IPC",
        pass: ipc.available === true && ipc.isRecording === false,
        evidence: "runtime-dev-report.json",
        ipc,
      },
      {
        id: "P1-DEV-NO-RUNTIME-CSP-ERRORS",
        pass:
          diagnostics.runtimeExceptions === 0 &&
          diagnostics.unexpectedConsoleErrors === 0 &&
          diagnostics.logErrors === 0 &&
          diagnostics.cspIpcEntries === 0,
        evidence: "03-dev-diagnostics.json",
        counts: {
          runtimeExceptions: diagnostics.runtimeExceptions,
          consoleErrors: diagnostics.consoleErrors,
          expectedEnvironmentConsoleErrors: diagnostics.expectedEnvironmentConsoleErrors,
          unexpectedConsoleErrors: diagnostics.unexpectedConsoleErrors,
          logErrors: diagnostics.logErrors,
          cspIpcEntries: diagnostics.cspIpcEntries,
        },
      },
    ],
  };
  report.finishedAt = new Date().toISOString();
  report.summary = {
    passed: report.checks.filter((check) => check.pass).length,
    failed: report.checks.filter((check) => !check.pass).length,
    total: report.checks.length,
  };
  await fs.writeFile(
    path.join(outputDirectory, "runtime-dev-report.json"),
    `${JSON.stringify(report, null, 2)}\n`,
  );
  return report;
}

async function main() {
  if (!["switch", "cold", "dev"].includes(mode)) {
    throw new Error(`Unsupported mode: ${mode}`);
  }
  await fs.mkdir(outputDirectory, { recursive: true });
  const target = await findTarget();
  const client = new CdpClient(target.webSocketDebuggerUrl);
  await client.connect();
  await Promise.all([
    client.send("Runtime.enable"),
    client.send("Page.enable"),
    client.send("Log.enable"),
    client.send("Network.enable"),
  ]);
  try {
    const report =
      mode === "switch"
        ? await runSwitchAudit(client, target)
        : mode === "cold"
          ? await runColdAudit(client, target)
          : await runDevAudit(client, target);
    process.stdout.write(`${JSON.stringify(report.summary)}\n`);
    if (report.summary.failed > 0) process.exitCode = 1;
  } finally {
    client.close();
  }
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exit(1);
});
