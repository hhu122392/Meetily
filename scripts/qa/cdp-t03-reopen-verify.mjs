import fs from "node:fs";

const meetingId = process.argv[2];
const screenshotPath = process.argv[3];
const expectedTitle = process.argv[4] ?? "QA-CORE-20260824-LIVE";
const savedMarker = process.argv[5] ?? "【本轮人工校正】";
const cancelledMarker = process.argv[6] ?? "【本轮不应保存】";
const outputPath = process.argv[7];
if (!meetingId) {
  console.error("Usage: node cdp-t03-reopen-verify.mjs <meeting-id> [screenshot.png]");
  process.exit(2);
}

const cdpPort = process.env.CDP_PORT ?? "9222";
const getPage = async () => {
  const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`).then((response) =>
    response.json(),
  );
  return targets.find(
    (target) => target.type === "page" && target.url.startsWith("http://tauri.localhost"),
  );
};
const page = await getPage();
if (!page) throw new Error("Meetily WebView2 release target was not found");

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
    throw new Error(evaluated.exceptionDetails.text ?? "Runtime.evaluate failed");
  }
  return evaluated.result.value;
}

await call("Runtime.enable");
await call("Page.enable");
await call("Emulation.setDeviceMetricsOverride", {
  width: 1600,
  height: 900,
  deviceScaleFactor: 1,
  mobile: false,
});
await call("Page.navigate", { url: "http://tauri.localhost/" });
await new Promise((resolve) => setTimeout(resolve, 1200));
const home = await evaluate(`({
  capturedAt: new Date().toISOString(),
  href: location.href,
  text: document.body.innerText,
})`);

const destination = `http://tauri.localhost/meeting-details?id=${encodeURIComponent(meetingId)}&source=recording`;
await call("Page.navigate", { url: destination });
await new Promise((resolve) => setTimeout(resolve, 2500));
const details = await evaluate(`(async () => {
  const meetingId = ${JSON.stringify(meetingId)};
  const savedMarker = ${JSON.stringify(savedMarker)};
  const cancelledMarker = ${JSON.stringify(cancelledMarker)};
  const meeting = await window.__TAURI_INTERNALS__.invoke('api_get_meeting', { meetingId });
  const segments = [...document.querySelectorAll('[data-transcript-id]')].map((segment) => ({
    id: segment.dataset.transcriptId,
    text: segment.innerText,
  }));
  return {
    capturedAt: new Date().toISOString(),
    href: location.href,
    titleText: document.querySelector('h1')?.textContent?.trim() ?? null,
    savedMarkerCountInDom: document.body.innerText.split(savedMarker).length - 1,
    cancelledMarkerCountInDom: document.body.innerText.split(cancelledMarker).length - 1,
    editButtonCount: [...document.querySelectorAll('button')].filter(
      (button) => button.getAttribute('aria-label') === '编辑转写片段',
    ).length,
    segments,
    meeting,
  };
})()`);

if (screenshotPath) {
  const screenshot = await call("Page.captureScreenshot", { format: "png" });
  fs.writeFileSync(screenshotPath, Buffer.from(screenshot.data, "base64"));
}

const report = {
      home,
      details,
      verdict: {
        leftMeetingPage: home.href === "http://tauri.localhost/",
        reopenedExpectedMeeting: details.href.includes(meetingId),
        titlePersisted: details.titleText === expectedTitle && details.meeting?.title === expectedTitle,
        savedMarkerPersisted: details.savedMarkerCountInDom === 1 && details.meeting?.transcripts?.filter((segment) => segment.text.includes(savedMarker)).length === 1,
        cancelledMarkerAbsent: details.cancelledMarkerCountInDom === 0 && !details.meeting?.transcripts?.some((segment) => segment.text.includes(cancelledMarker)),
        editControlsAvailable: details.editButtonCount === details.meeting?.transcripts?.length,
      },
    };
const output = `${JSON.stringify(report, null, 2)}\n`;
if (outputPath) fs.writeFileSync(outputPath, output, "utf8");
console.log(output.trimEnd());
socket.close();
