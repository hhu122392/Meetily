import type { SummaryFieldTrace } from '@/types/summary-source';

export interface EvidenceTranscript { id: string; text: string }
export interface ResolvedSummaryTrace {
  trace: SummaryFieldTrace;
  sources: Array<{ segmentId: string; text: string; startMs: number | null; related: boolean }>;
}

export async function resolveSummaryEvidence(
  traces: readonly SummaryFieldTrace[],
  transcripts: readonly EvidenceTranscript[],
): Promise<ResolvedSummaryTrace[]> {
  const byId = new Map(transcripts.map(transcript => [transcript.id, transcript]));
  const digests = new Map<string, Promise<string>>();
  const digestFor = (id: string, text: string) => {
    if (!digests.has(id)) {
      digests.set(id, crypto.subtle.digest('SHA-256', new TextEncoder().encode(text.trim()))
        .then(buffer => Array.from(new Uint8Array(buffer), byte => byte.toString(16).padStart(2, '0')).join('')));
    }
    return digests.get(id)!;
  };
  return Promise.all(traces.map(async trace => {
    const sources: ResolvedSummaryTrace['sources'] = [];
    const related = trace.status !== 'supported';
    const references = related ? trace.relatedEvidence ?? [] : trace.evidence;
    for (const reference of references) {
      const transcript = byId.get(reference.segmentId);
      if (!transcript || await digestFor(transcript.id, transcript.text) !== reference.excerptSha256) continue;
      sources.push({
        segmentId: transcript.id, text: transcript.text,
        startMs: reference.startMs, related,
      });
    }
    return { trace, sources };
  }));
}
