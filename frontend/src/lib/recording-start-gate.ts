export type IsRecordingCheck = () => Promise<boolean>;
export type StartRecordingAction = () => Promise<void>;

/**
 * Serializes all frontend recording-start entry points.
 *
 * The backend does several seconds of model/device initialization before its
 * recording flag becomes true. Without a shared gate, a second click or a
 * second UI trigger can enter during that window and later surface a false
 * "Recording already in progress" error after the first request succeeds.
 */
export class RecordingStartGate {
  private inFlight: Promise<void> | null = null;

  run(isRecording: IsRecordingCheck, start: StartRecordingAction): Promise<void> {
    if (this.inFlight) {
      return this.inFlight;
    }

    const operation = (async () => {
      // Treat a duplicate request after a successful start as idempotent
      // success. A genuine later start remains possible once recording stops.
      if (await isRecording()) {
        return;
      }

      await start();
    })();

    this.inFlight = operation;
    void operation.finally(() => {
      if (this.inFlight === operation) {
        this.inFlight = null;
      }
    }).catch(() => {
      // The original operation is returned to the caller and retains the
      // rejection. This branch only handles the promise created by finally().
    });

    return operation;
  }
}
