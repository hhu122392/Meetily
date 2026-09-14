import fs from 'node:fs';

const [identityPath, firstLaunchPath, armedPath, restartPath, rejectionPath] = process.argv.slice(2);
if (!rejectionPath) {
  throw new Error(
    'Usage: node audit-t09-autostart-restart.mjs <identity> <first-launch> <armed> <restart> <rejection>',
  );
}
const read = (path) => JSON.parse(fs.readFileSync(path, 'utf8'));
const identity = read(identityPath);
const firstLaunch = read(firstLaunchPath);
const armed = read(armedPath);
const restart = read(restartPath);
const rejection = read(rejectionPath);

const requestedAt = Number(armed.request.requestedAt);
const restartedAt = Date.parse(restart.restartedAt);
const firstObservedAt = Date.parse(rejection.samples[0].at);
const processRestartDelayMs = restartedAt - requestedAt;
const firstObservationAgeMs = firstObservedAt - requestedAt;
const verdict = {
  formalArtifactMatchedOnBothLaunches:
    identity.sha256 === firstLaunch.sha256 && identity.sha256 === restart.sha256,
  oldProcessExitedBeforeRestart: restart.oldExited === true,
  requestWasCurrentVersionAndProcessBound:
    armed.request.version === 3
    && typeof armed.request.rendererSessionId === 'string'
    && armed.request.rendererSessionId.length > 0
    && typeof armed.request.appSessionId === 'string'
    && armed.request.appSessionId.length > 0,
  restartedWhileRequestWasWithinTtl:
    processRestartDelayMs >= 0 && processRestartDelayMs <= 15_000,
  nativeProcessSessionChanged:
    armed.request.appSessionId !== rejection.appSessionId,
  staleRequestCleared: rejection.verdict.staleRequestCleared === true,
  neverRecorded: rejection.verdict.neverRecorded === true,
  neverTranscribed: rejection.verdict.neverTranscribed === true,
  noRecordingUi: rejection.verdict.noRecordingUi === true,
};

console.log(JSON.stringify({
  auditedAt: new Date().toISOString(),
  identityPath,
  firstLaunchPath,
  armedPath,
  restartPath,
  rejectionPath,
  timing: {
    requestedAt: new Date(requestedAt).toISOString(),
    restartedAt: new Date(restartedAt).toISOString(),
    firstObservedAt: new Date(firstObservedAt).toISOString(),
    processRestartDelayMs,
    firstObservationAgeMs,
    ttlMs: 15_000,
    observationNote: firstObservationAgeMs > 15_000
      ? 'The full app became CDP-observable after the TTL, but the native process was already restarted inside the TTL.'
      : 'The first runtime observation was also inside the TTL.',
  },
  appSessions: {
    beforeRestart: armed.request.appSessionId,
    afterRestart: rejection.appSessionId,
  },
  verdict,
  pass: Object.values(verdict).every(Boolean),
}, null, 2));
