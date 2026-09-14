const meetingId = process.argv[2];
const meetingTitle = process.argv[3];
const mode = process.argv[4] ?? 'summary';
if (!meetingId || !meetingTitle) {
  throw new Error("Usage: node cdp-ui-open-meeting.mjs <meeting-id> <meeting-title>");
}
const cdpPort = process.env.CDP_PORT ?? "9233";
const targets = await fetch(`http://127.0.0.1:${cdpPort}/json/list`).then((response) => response.json());
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
await call("Runtime.enable");
const expression = `(async () => {
  const meetingId = ${JSON.stringify(meetingId)};
  const meetingTitle = ${JSON.stringify(meetingTitle)};
  const mode = ${JSON.stringify(mode)};
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const isVisible = (element) => {
    const rect = element.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const findMeetingCandidates = () => [...document.querySelectorAll('button, a, [role="button"], div, span, p')]
    .filter((element) => isVisible(element) && element.innerText?.trim() === meetingTitle)
    .sort((left, right) => left.childElementCount - right.childElementCount);
  let candidates = findMeetingCandidates();
  let sidebarExpansion = null;
  if (candidates.length === 0) {
    const meetingNotesButton = [...document.querySelectorAll('button')].find((button) =>
      isVisible(button)
      && (button.getAttribute('aria-label') ?? '').trim() === '会议记录'
    );
    if (meetingNotesButton) {
      sidebarExpansion = {
        clickedAt: new Date().toISOString(),
        ariaLabel: meetingNotesButton.getAttribute('aria-label'),
        outerHTML: meetingNotesButton.outerHTML.slice(0, 1000),
      };
      meetingNotesButton.click();
      const expansionDeadline = performance.now() + 5000;
      while (performance.now() < expansionDeadline) {
        await sleep(100);
        candidates = findMeetingCandidates();
        if (candidates.length > 0) break;
      }
    }
  }
  if (candidates.length === 0) throw new Error('Visible meeting title was not found on home/sidebar UI');
  const selected = candidates[0];
  const selectedEvidence = {
    tagName: selected.tagName,
    className: String(selected.className),
    role: selected.getAttribute('role'),
    outerHTML: selected.outerHTML.slice(0, 2000),
  };
  selected.click();
  const samples = [];
  const deadline = performance.now() + 10000;
  while (performance.now() < deadline) {
    const body = document.body?.innerText ?? '';
    samples.push({
      at: new Date().toISOString(),
      href: location.href,
      hasTargetMeeting: location.href.includes(meetingId),
      hasSavedMarker: body.includes('【摘要人工校正】'),
      hasEditor: [...document.querySelectorAll('[contenteditable="true"]')]
        .some((element) => isVisible(element) && element.innerText.includes('M100')),
    });
    await sleep(250);
  }
  const body = document.body?.innerText ?? '';
  const importedNotPolluted = !body.includes('【摘要人工校正】')
    && !body.includes('【重启复核】');
  const summaryEditorReady = [...document.querySelectorAll('[contenteditable="true"]')]
    .some((element) => isVisible(element) && element.innerText.includes('M100'));
  return {
    clickedAt: samples[0]?.at ?? new Date().toISOString(),
    sidebarExpansion,
    selectedEvidence,
    candidateCount: candidates.length,
    finalHref: location.href,
    targetMeetingStableForLastFiveSeconds: samples.slice(-20).every((sample) => sample.hasTargetMeeting),
    savedMarkerVisible: body.includes('【摘要人工校正】'),
    editorVisible: summaryEditorReady,
    importedNotPolluted,
    samples,
    verdict: {
      oneMeetingClickIssued: true,
      targetMeetingReached: location.href.includes(meetingId),
      stableForFiveSeconds: samples.slice(-20).every((sample) => sample.hasTargetMeeting),
      savedSummaryVisible: body.includes('【摘要人工校正】'),
      summaryEditorReady,
      importedNotPolluted,
      expectedContentState: mode === 'imported'
        ? importedNotPolluted
        : body.includes('【摘要人工校正】') && summaryEditorReady,
    },
  };
})()`;
const evaluated = await call("Runtime.evaluate", {
  expression,
  returnByValue: true,
  awaitPromise: true,
  userGesture: true,
});
if (evaluated.exceptionDetails) {
  throw new Error(evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text);
}
console.log(JSON.stringify(evaluated.result.value, null, 2));
socket.close();
