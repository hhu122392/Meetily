const liveMeetingId = process.argv[2];
const importedMeetingId = process.argv[3];
const sourceAudioPath = process.argv[4];
const importedAudioPath = process.argv[5];
if (!liveMeetingId || !importedMeetingId || !sourceAudioPath || !importedAudioPath) {
  throw new Error(
    "Usage: node cdp-t04-audit.mjs <live-meeting-id> <imported-meeting-id> <source-audio> <imported-audio>",
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

await call("Runtime.enable");
const expression = `(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke;
  const [liveMeeting, importedMeeting, sourceAudio, importedAudio] = await Promise.all([
    invoke('api_get_meeting', { meetingId: ${JSON.stringify(liveMeetingId)} }),
    invoke('api_get_meeting', { meetingId: ${JSON.stringify(importedMeetingId)} }),
    invoke('validate_audio_file_command', { path: ${JSON.stringify(sourceAudioPath)} }),
    invoke('validate_audio_file_command', { path: ${JSON.stringify(importedAudioPath)} }),
  ]);
  const savedMarker = '【人工校正】';
  return {
    capturedAt: new Date().toISOString(),
    href: location.href,
    liveMeeting,
    importedMeeting,
    sourceAudio,
    importedAudio,
    verdict: {
      distinctMeetingIds: liveMeeting.id !== importedMeeting.id,
      importedTitleCorrect: importedMeeting.title === 'QA-CORE-20260824-IMPORT',
      liveSavedMarkerCount: liveMeeting.transcripts.filter((segment) => segment.text.includes(savedMarker)).length,
      importedSavedMarkerCount: importedMeeting.transcripts.filter((segment) => segment.text.includes(savedMarker)).length,
      durationEqual: sourceAudio.duration_seconds === importedAudio.duration_seconds,
      sizeEqual: sourceAudio.size_bytes === importedAudio.size_bytes,
      formatEqual: sourceAudio.format === importedAudio.format,
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
