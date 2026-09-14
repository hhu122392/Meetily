#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

const port = Number(process.argv[2] || 9333);
const outputDirectory = path.resolve(
  process.argv[3] || "docs/i18n/audit/phase-2-react/2A/runtime",
);

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

async function findTarget() {
  let lastError;
  for (let attempt = 0; attempt < 80; attempt += 1) {
    try {
      const targets = await fetch("http://127.0.0.1:" + port + "/json").then(
        (response) => response.json(),
      );
      const target = targets.find((candidate) => candidate.type === "page");
      if (target?.webSocketDebuggerUrl) return target;
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error("No page target on port " + port + ": " + String(lastError || "timeout"));
}

async function evaluate(client, expression) {
  const result = await client.send("Runtime.evaluate", {
    expression,
    awaitPromise: true,
    returnByValue: true,
    userGesture: true,
  });
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
  }
  return result.result.value;
}

async function waitFor(client, expression, description, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await evaluate(client, expression)) return;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error("Timed out waiting for " + description);
}

async function waitForReady(client) {
  await waitFor(
    client,
    "Boolean(document.body && document.querySelector('[data-i18n-ready=\"true\"]'))",
    "the i18n provider and body",
  );
}

async function setLocale(client, locale) {
  await evaluate(
    client,
    "localStorage.setItem('meetily.uiLocale'," + JSON.stringify(locale) + ")",
  );
  await client.send("Page.reload", { ignoreCache: true });
  await waitForReady(client);
}

async function clickText(client, text) {
  const point = await evaluate(
    client,
    "(() => {" +
      "const wanted=" + JSON.stringify(text) + ";" +
      "const candidates=[...document.querySelectorAll('button,a')];" +
      "const element=candidates.find((item)=>{" +
        "const rect=item.getBoundingClientRect();" +
        "return (item.textContent.trim()===wanted||item.getAttribute('aria-label')===wanted)&&!item.disabled&&rect.width>0&&rect.height>0;" +
      "});" +
      "if(!element) return null;" +
      "const rect=element.getBoundingClientRect();" +
      "return {x:rect.left+rect.width/2,y:rect.top+rect.height/2,disabled:Boolean(element.disabled)};" +
    "})()",
  );
  if (!point || point.disabled) {
    throw new Error("Clickable text not found or disabled: " + text);
  }
  await client.send("Input.dispatchMouseEvent", {
    type: "mousePressed",
    x: point.x,
    y: point.y,
    button: "left",
    clickCount: 1,
  });
  await client.send("Input.dispatchMouseEvent", {
    type: "mouseReleased",
    x: point.x,
    y: point.y,
    button: "left",
    clickCount: 1,
  });
}

async function invoke(client, command, args = {}) {
  return evaluate(
    client,
    "(async()=>{" +
      "try{return {resolved:true,value:await window.__TAURI_INTERNALS__.invoke(" +
        JSON.stringify(command) + "," + JSON.stringify(args) +
      ")}}catch(error){return {resolved:false,error:String(error)}};" +
    "})()",
  );
}

async function snapshot(client, name) {
  // Let Framer Motion entrance transitions settle so visual evidence reflects
  // the stable UI rather than a partially transparent animation frame.
  await new Promise((resolve) => setTimeout(resolve, 750));
  const state = await evaluate(
    client,
    "(() => {" +
      "const controls=[...document.querySelectorAll('button,a')].map((element)=>{" +
        "const rect=element.getBoundingClientRect();" +
        "return {text:element.textContent.trim(),ariaLabel:element.getAttribute('aria-label')," +
          "disabled:Boolean(element.disabled),visible:rect.width>0&&rect.height>0," +
          "rect:{left:rect.left,top:rect.top,right:rect.right,bottom:rect.bottom}};" +
      "});" +
      "const visibleControls=controls.filter((item)=>item.visible);" +
      "const clippedControls=visibleControls.filter((item)=>item.rect.left<0||item.rect.top<0||" +
        "item.rect.right>innerWidth+1||item.rect.bottom>innerHeight+1);" +
      "return {url:location.href,htmlLang:document.documentElement.lang," +
        "htmlDir:document.documentElement.dir,bodyText:document.body.innerText," +
        "viewport:{width:innerWidth,height:innerHeight}," +
        "documentSize:{width:document.documentElement.scrollWidth,height:document.documentElement.scrollHeight}," +
        "controls:visibleControls,clippedControls," +
        "i18nReady:document.querySelector('[data-i18n-ready]')?.getAttribute('data-i18n-ready')};" +
    "})()",
  );
  const image = await client.send("Page.captureScreenshot", {
    format: "png",
    captureBeyondViewport: false,
  });
  await Promise.all([
    fs.writeFile(path.join(outputDirectory, name + ".json"), JSON.stringify(state, null, 2) + "\n"),
    fs.writeFile(path.join(outputDirectory, name + ".png"), Buffer.from(image.data, "base64")),
  ]);
  return state;
}

function includesAll(text, expected) {
  return expected.every((value) => text.includes(value));
}

function diagnostics(events) {
  const exceptions = events.filter((event) => event.method === "Runtime.exceptionThrown");
  const consoleErrors = events.filter(
    (event) =>
      event.method === "Runtime.consoleAPICalled" &&
      event.params?.type === "error",
  );
  const logErrors = events.filter(
    (event) =>
      event.method === "Log.entryAdded" &&
      event.params?.entry?.level === "error",
  );
  const serialized = JSON.stringify(events);
  const csp = (serialized.match(/content security policy|blocked by csp/gi) || []).length;
  const consoleEventText = (event) =>
    (event.params?.args || [])
      .map((argument) => argument.value || argument.description || "")
      .join(" | ");
  const expectedConsoleErrors = consoleErrors.filter((event) => {
    const message = consoleEventText(event);
    return (
      /PHASE2_INTERNAL_(?:PARAKEET|SUMMARY)_ERROR_SENTINEL/.test(message) ||
      /Download cancelled by user|CANCELLED: Download cancelled by user/.test(message) ||
      (/\[OnboardingContext\] Parakeet download failed:|Parakeet download error:/.test(message) &&
        /Failed to create file encoder-model\.int8\.onnx/.test(message))
    );
  });
  const expectedSet = new Set(expectedConsoleErrors);
  const unexpectedConsoleErrors = consoleErrors.filter((event) => !expectedSet.has(event));
  return {
    exceptions: exceptions.length,
    consoleErrors: consoleErrors.length,
    expectedConsoleErrors: expectedConsoleErrors.length,
    unexpectedConsoleErrors: unexpectedConsoleErrors.length,
    logErrors: logErrors.length,
    csp,
    exceptionEvents: exceptions,
    consoleErrorEvents: consoleErrors,
    expectedConsoleErrorEvents: expectedConsoleErrors,
    unexpectedConsoleErrorEvents: unexpectedConsoleErrors,
    logErrorEvents: logErrors,
  };
}

async function main() {
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
    await waitForReady(client);
    await setLocale(client, "zh-CN");
    await waitFor(client, "document.body.innerText.includes('欢迎使用 Meetily')", "Chinese welcome");
    const zhWelcome = await snapshot(client, "01-welcome-zh-CN");

    await setLocale(client, "en");
    await waitFor(client, "document.body.innerText.includes('Welcome to Meetily')", "English welcome");
    const enWelcome = await snapshot(client, "02-welcome-en");

    await setLocale(client, "zh-CN");
    await clickText(client, "开始设置");
    await waitFor(client, "document.body.innerText.includes('设置概览')", "Chinese setup overview");
    const setup = await snapshot(client, "03-setup-overview-zh-CN");

    await clickText(client, "开始下载");
    await waitFor(client, "document.body.innerText.includes('正在进行初始设置')", "download step");

    // Freeze the live network-driven progress stream before injecting deterministic
    // values. Otherwise a real progress event can overwrite the fixture between the
    // emit and the settled visual snapshot, making the audit depend on network speed.
    // Give both effect-driven download commands time to register their cancellation
    // handles before invoking cancel, then let their terminal events drain.
    await new Promise((resolve) => setTimeout(resolve, 750));
    const recommended = await invoke(client, "builtin_ai_get_recommended_model");
    const freezeParakeet = await invoke(client, "parakeet_cancel_download", {
      modelName: "parakeet-tdt-0.6b-v3-int8",
    });
    const freezeSummary =
      recommended.resolved && recommended.value
        ? await invoke(client, "builtin_ai_cancel_download", {
            modelName: recommended.value,
          })
        : { resolved: false, error: "No recommended summary model" };
    await new Promise((resolve) => setTimeout(resolve, 1500));

    await invoke(client, "plugin:event|emit", {
      event: "parakeet-model-download-progress",
      payload: {
        modelName: "parakeet-tdt-0.6b-v3-int8",
        progress: 42,
        downloaded_mb: 281.4,
        total_mb: 670,
        speed_mbps: 12.5,
        status: "downloading"
      }
    });
    if (recommended.resolved && recommended.value) {
      await invoke(client, "plugin:event|emit", {
        event: "builtin-ai-download-progress",
        payload: {
          model: recommended.value,
          progress: 35,
          downloaded_mb: 427.4,
          total_mb: 1221,
          speed_mbps: 8.2,
          status: "downloading"
        }
      });
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
    const progress = await snapshot(client, "04-download-progress-zh-CN");

    const parakeetSentinel = "PHASE2_INTERNAL_PARAKEET_ERROR_SENTINEL";
    const summarySentinel = "PHASE2_INTERNAL_SUMMARY_ERROR_SENTINEL";
    await invoke(client, "plugin:event|emit", {
      event: "parakeet-model-download-error",
      payload: {
        modelName: "parakeet-tdt-0.6b-v3-int8",
        error: parakeetSentinel
      }
    });
    if (recommended.resolved && recommended.value) {
      await invoke(client, "plugin:event|emit", {
        event: "builtin-ai-download-progress",
        payload: {
          model: recommended.value,
          progress: 35,
          status: "error",
          error: summarySentinel
        }
      });
    }
    await waitFor(client, "document.body.innerText.includes('下载错误')", "localized download error");
    const failure = await snapshot(client, "05-download-failure-zh-CN");

    await clickText(client, "重试");
    await waitFor(
      client,
      "!document.body.innerText.includes('PHASE2_INTERNAL_PARAKEET_ERROR_SENTINEL')",
      "safe retry state",
    );
    await new Promise((resolve) => setTimeout(resolve, 500));
    const retry = await snapshot(client, "06-download-retry-zh-CN");

    const cancelParakeet = await invoke(client, "parakeet_cancel_download", {
      modelName: "parakeet-tdt-0.6b-v3-int8",
    });
    const cancelSummary =
      recommended.resolved && recommended.value
        ? await invoke(client, "builtin_ai_cancel_download", {
            modelName: recommended.value,
          })
        : { resolved: false, error: "No recommended summary model" };

    // Verify the success terminal state through the same Tauri event bridge used
    // by the real download commands, after all live network activity is stopped.
    await new Promise((resolve) => setTimeout(resolve, 500));
    const hasRetryToast = await evaluate(
      client,
      "Boolean([...document.querySelectorAll('button')].find((item)=>item.getAttribute('aria-label')==='关闭通知'))",
    );
    if (hasRetryToast) {
      await clickText(client, "关闭通知");
    }
    await invoke(client, "plugin:event|emit", {
      event: "parakeet-model-download-progress",
      payload: {
        modelName: "parakeet-tdt-0.6b-v3-int8",
        progress: 100,
        downloaded_mb: 670,
        total_mb: 670,
        speed_mbps: 0,
        status: "completed",
      },
    });
    await invoke(client, "plugin:event|emit", {
      event: "parakeet-model-download-complete",
      payload: { modelName: "parakeet-tdt-0.6b-v3-int8" },
    });
    if (recommended.resolved && recommended.value) {
      await invoke(client, "plugin:event|emit", {
        event: "builtin-ai-download-progress",
        payload: {
          model: recommended.value,
          progress: 100,
          downloaded_mb: 2614,
          total_mb: 2614,
          speed_mbps: 0,
          status: "completed",
        },
      });
    }
    const completion = await snapshot(client, "07-download-complete-zh-CN");

    const runtimeDiagnostics = diagnostics(client.events);
    await fs.writeFile(
      path.join(outputDirectory, "07-runtime-diagnostics.json"),
      JSON.stringify(runtimeDiagnostics, null, 2) + "\n",
    );

    const checks = [
      {
        id: "P2A-RUN-ZH-WELCOME",
        pass:
          zhWelcome.htmlLang === "zh-CN" &&
          includesAll(zhWelcome.bodyText, [
            "欢迎使用 Meetily",
            "数据始终留在你的设备上",
            "智能摘要与洞察",
            "离线运行，无需云服务",
            "开始设置",
          ]) &&
          !zhWelcome.bodyText.includes("Get Started"),
        evidence: "01-welcome-zh-CN.json/png",
      },
      {
        id: "P2A-RUN-EN-WELCOME",
        pass:
          enWelcome.htmlLang === "en" &&
          includesAll(enWelcome.bodyText, [
            "Welcome to Meetily",
            "Your data never leaves your device",
            "Get Started",
          ]),
        evidence: "02-welcome-en.json/png",
      },
      {
        id: "P2A-RUN-ZH-SETUP",
        pass:
          includesAll(setup.bodyText, [
            "设置概览",
            "第 1 步：下载转录引擎",
            "第 2 步：下载摘要引擎",
            "开始下载",
          ]) && !setup.bodyText.includes("Setup Overview"),
        evidence: "03-setup-overview-zh-CN.json/png",
      },
      {
        id: "P2A-RUN-ZH-PROGRESS-AND-FORMATTING",
        pass:
          includesAll(progress.bodyText, [
            "正在进行初始设置",
            "转录引擎",
            "摘要引擎",
            "281.4 MB / 670.0 MB",
            "42%",
          ]) && !progress.bodyText.includes("Transcription Engine"),
        evidence: "04-download-progress-zh-CN.json/png",
      },
      {
        id: "P2A-RUN-SAFE-LOCALIZED-FAILURE",
        pass:
          includesAll(failure.bodyText, ["下载错误", "请检查网络连接后重试。", "重试"]) &&
          !failure.bodyText.includes(parakeetSentinel) &&
          !failure.bodyText.includes(summarySentinel) &&
          !failure.bodyText.includes("Download Error"),
        evidence: "05-download-failure-zh-CN.json/png",
      },
      {
        id: "P2A-RUN-RETRY-ACTION",
        pass:
          !retry.bodyText.includes(parakeetSentinel) &&
          !retry.bodyText.includes("Retry failed"),
        evidence: "06-download-retry-zh-CN.json/png",
      },
      {
        id: "P2A-RUN-SUCCESS-TERMINAL-STATE",
        pass:
          includesAll(completion.bodyText, ["转录引擎", "摘要引擎", "100%", "继续"]) &&
          (completion.bodyText.match(/100%/g) || []).length >= 2 &&
          completion.controls.some((control) => control.text === "继续" && !control.disabled) &&
          !completion.bodyText.includes("下载错误") &&
          !completion.bodyText.includes("Completed"),
        evidence: "07-download-complete-zh-CN.json/png",
      },
      {
        id: "P2A-RUN-A11Y-AND-LAYOUT",
        pass:
          [zhWelcome, enWelcome, setup, progress, failure, retry, completion].every(
            (state) =>
              state.clippedControls.every(
                (control) =>
                  control.rect.left >= 0 &&
                  control.rect.right <= state.viewport.width + 1,
              ),
          ) &&
          setup.controls.some((control) => control.ariaLabel?.includes("初始设置第")),
        evidence: "01-07 snapshots",
      },
      {
        id: "P2A-RUN-NO-RUNTIME-OR-CSP-ERRORS",
        pass:
          runtimeDiagnostics.exceptions === 0 &&
          runtimeDiagnostics.unexpectedConsoleErrors === 0 &&
          runtimeDiagnostics.logErrors === 0 &&
          runtimeDiagnostics.csp === 0,
        evidence: "07-runtime-diagnostics.json",
        diagnostics: runtimeDiagnostics,
      },
      {
        id: "P2A-RUN-DOWNLOAD-CLEANUP",
        pass:
          freezeParakeet.resolved === true &&
          (freezeSummary.resolved === true || !recommended.resolved) &&
          cancelParakeet.resolved === true,
        evidence: "runtime report",
        freezeParakeet,
        freezeSummary,
        cancelParakeet,
        cancelSummary,
      },
    ];

    const report = {
      phase: "15.5-stage-2-react-frontend-migration",
      batch: "2A",
      scope: "first-run-permissions-model-download",
      generatedAt: new Date().toISOString(),
      target: { title: target.title, url: target.url },
      checks,
      summary: {
        passed: checks.filter((check) => check.pass).length,
        failed: checks.filter((check) => !check.pass).length,
        total: checks.length,
      },
    };
    await fs.writeFile(
      path.join(outputDirectory, "runtime-2A-report.json"),
      JSON.stringify(report, null, 2) + "\n",
    );
    process.stdout.write(JSON.stringify(report.summary) + "\n");
    if (report.summary.failed) process.exitCode = 1;
  } finally {
    client.close();
  }
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exit(1);
});
