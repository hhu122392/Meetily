export interface NativeSummarySnapshot {
  status: string;
  start?: string | null;
  data?: Record<string, unknown> | null;
}

export interface ResumedSummaryUpdate {
  status: 'idle' | 'processing' | 'regenerating' | 'completed' | 'needs_review' | 'error';
  error: 'generationFailed' | 'emptyContent' | null;
  data?: Record<string, unknown>;
  startedAt?: number;
}

function isActive(status: string): boolean {
  return ['pending', 'processing', 'summarizing', 'regenerating'].includes(status);
}

function toUpdate(snapshot: NativeSummarySnapshot): ResumedSummaryUpdate {
  const data = snapshot.data && Object.keys(snapshot.data).length ? snapshot.data : undefined;
  const validation = data?.factValidation as { status?: string } | undefined;
  const completed = validation?.status === 'needs_review' ? 'needs_review' : 'completed';
  if (isActive(snapshot.status)) {
    // The existing editor already contains the saved report. Do not replace it
    // with backup data on every status check while the new report is pending.
    const startedAt = typeof snapshot.start === 'string' ? Date.parse(snapshot.start) : NaN;
    return {
      status: data ? 'regenerating' : 'processing', error: null,
      ...(Number.isFinite(startedAt) ? { startedAt } : {}),
    };
  }
  if (snapshot.status === 'completed') {
    return data ? { status: completed, error: null, data } : { status: 'error', error: 'emptyContent' };
  }
  if (snapshot.status === 'cancelled' || snapshot.status === 'idle') {
    return data ? { status: completed, error: null, data } : { status: 'idle', error: null };
  }
  return { status: 'error', error: 'generationFailed', ...(data ? { data } : {}) };
}

/** Resume an existing native job, or a newer result completed during page load.
 * Stopping this observer never cancels the native generation. */
export function observeExistingSummaryTask({
  read,
  onUpdate,
  getDisplayedGenerationId,
  isSuperseded = () => false,
  intervalMs = 2000,
}: {
  read: () => Promise<NativeSummarySnapshot>;
  onUpdate: (update: ResumedSummaryUpdate) => void;
  getDisplayedGenerationId?: () => string | undefined;
  isSuperseded?: () => boolean;
  intervalMs?: number;
}): () => void {
  let stopped = false;
  let resumed = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const inactive = () => stopped || isSuperseded();
  const check = async () => {
    if (inactive()) return;
    try {
      const snapshot = await read();
      if (inactive()) return;
      if (!resumed && !isActive(snapshot.status)) {
        const generationId = (snapshot.data?.template_snapshot as { generationId?: unknown } | undefined)?.generationId;
        // Same-generation reads must not replace the user's unsaved edits.
        if (snapshot.status !== 'completed' || typeof generationId !== 'string' || !generationId
          || !getDisplayedGenerationId || generationId === getDisplayedGenerationId()) return;
      }
      resumed = true;
      onUpdate(toUpdate(snapshot));
      // Schedule after the read settles; slow reads must never overlap.
      if (isActive(snapshot.status) && !inactive()) timer = setTimeout(() => void check(), intervalMs);
    } catch {
      if (!inactive()) onUpdate({ status: 'error', error: 'generationFailed' });
    }
  };
  void check();
  return () => {
    stopped = true;
    if (timer !== undefined) clearTimeout(timer);
  };
}
