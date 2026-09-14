const AUTO_START_RECORDING_KEY = 'autoStartRecording';

// sessionStorage can survive a WebView/app process restart on Windows. A TTL
// alone is therefore insufficient: a request written shortly before exit can
// be consumed by the next renderer and start recording without a fresh user
// action. WebView2 may also restore the same renderer/browser session, so the
// renderer id alone is not a process boundary. Every request is therefore
// bound to both the renderer instance and the native Rust process session.
const RENDERER_SESSION_ID = `${Date.now()}-${Math.random().toString(36).slice(2)}`;

// Navigation should mount the recording page almost immediately. Keeping this
// request short-lived prevents an abandoned sessionStorage flag from starting
// a recording much later, without a fresh user action.
export const AUTO_START_RECORDING_TTL_MS = 15_000;

type AutoStartStorage = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;

interface AutoStartRecordingRequest {
  version: 3;
  requestedAt: number;
  rendererSessionId: string;
  appSessionId: string;
}

export function queueAutoStartRecordingRequest(
  storage: AutoStartStorage,
  appSessionId: string,
  now = Date.now(),
  rendererSessionId = RENDERER_SESSION_ID,
): void {
  const request: AutoStartRecordingRequest = {
    version: 3,
    requestedAt: now,
    rendererSessionId,
    appSessionId,
  };

  storage.setItem(AUTO_START_RECORDING_KEY, JSON.stringify(request));
}

export function consumeAutoStartRecordingRequest(
  storage: AutoStartStorage,
  appSessionId: string,
  now = Date.now(),
  rendererSessionId = RENDERER_SESSION_ID,
): boolean {
  const rawRequest = storage.getItem(AUTO_START_RECORDING_KEY);
  if (rawRequest === null) {
    return false;
  }

  // A request is single-use regardless of whether it is valid. This also
  // clears legacy values such as the old literal string "true".
  storage.removeItem(AUTO_START_RECORDING_KEY);

  try {
    const request = JSON.parse(rawRequest) as Partial<AutoStartRecordingRequest>;
    if (
      request.version !== 3
      || !Number.isFinite(request.requestedAt)
      || request.rendererSessionId !== rendererSessionId
      || request.appSessionId !== appSessionId
    ) {
      return false;
    }

    const age = now - request.requestedAt!;
    return age >= 0 && age <= AUTO_START_RECORDING_TTL_MS;
  } catch {
    return false;
  }
}
