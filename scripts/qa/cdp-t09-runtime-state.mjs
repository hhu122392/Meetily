const liveMeetingId = process.argv[2];
const importedMeetingId = process.argv[3];
if (!liveMeetingId || !importedMeetingId) {
  throw new Error("Usage: node cdp-t09-runtime-state.mjs <live-id> <import-id>");
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
  const liveMeetingId = ${JSON.stringify(liveMeetingId)};
  const importedMeetingId = ${JSON.stringify(importedMeetingId)};
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const [isRecording, isPaused, recordingState, meetingFolder, transcriptionStatus,
    reconnectionStatus, meetings, liveSummary, liveHistory, liveManual, importedHistory] = await Promise.all([
    invoke('is_recording', {}),
    invoke('is_recording_paused', {}),
    invoke('get_recording_state', {}),
    invoke('get_meeting_folder_path', {}),
    invoke('get_transcription_status', {}),
    invoke('get_reconnection_status', {}),
    invoke('api_get_meetings', {}),
    invoke('api_get_summary', { meetingId: liveMeetingId }),
    invoke('api_list_summary_generation_history', { meetingId: liveMeetingId }),
    invoke('api_list_manual_summary_revisions', { meetingId: liveMeetingId }),
    invoke('api_list_summary_generation_history', { meetingId: importedMeetingId }),
  ]);
  const body = document.body?.innerText ?? '';
  return {
    capturedAt: new Date().toISOString(),
    href: location.href,
    isRecording,
    isPaused,
    recordingState,
    meetingFolder,
    transcriptionStatus,
    reconnectionStatus,
    meetings,
    liveSummary,
    liveHistory,
    liveManual,
    importedHistory,
    visibleAlerts: [...document.querySelectorAll('[role="alert"]')]
      .filter((element) => {
        const rect = element.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      })
      .map((element) => element.innerText.trim()),
    bodyHead: body.slice(0, 5000),
    verdict: {
      runtimeIdle: isRecording === false
        && isPaused === false
        && recordingState?.is_recording === false
        && recordingState?.is_active === false
        && meetingFolder === null
        && transcriptionStatus?.is_processing === false
        && transcriptionStatus?.chunks_in_queue === 0,
      liveMeetingPresentOnce: meetings.filter((meeting) => meeting.id === liveMeetingId).length === 1,
      importedMeetingPresentOnce: meetings.filter((meeting) => meeting.id === importedMeetingId).length === 1,
      liveSummaryCompleted: liveSummary?.status === 'completed',
      savedSummaryMarkerPresent: liveSummary?.data?.markdown?.includes('【摘要人工校正】') === true,
      liveHistoryCountFive: liveHistory.length === 5,
      manualRevisionCountOne: liveManual.length === 1,
      importedHasNoGenerationHistory: importedHistory.length === 0,
      noVisibleErrors: !body.includes('录音失败')
        && !body.includes('生成摘要时出错')
        && !body.includes('无法完成此操作'),
    },
  };
})()`;
const evaluated = await call("Runtime.evaluate", {
  expression,
  returnByValue: true,
  awaitPromise: true,
});
if (evaluated.exceptionDetails) {
  throw new Error(evaluated.exceptionDetails.exception?.description ?? evaluated.exceptionDetails.text);
}
console.log(JSON.stringify(evaluated.result.value, null, 2));
socket.close();
