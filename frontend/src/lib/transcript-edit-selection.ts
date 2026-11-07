import type { ProofreadCandidate } from './transcript-revision';

export function transcriptCandidateKey(candidate: ProofreadCandidate): string {
  return JSON.stringify([
    candidate.segment_id, candidate.start_char, candidate.end_char,
    candidate.original, candidate.suggested,
  ]);
}

export function hasConflictingTranscriptEdits(candidates: ProofreadCandidate[]): boolean {
  const bySegment = new Map<string, ProofreadCandidate[]>();
  for (const candidate of candidates) {
    const list = bySegment.get(candidate.segment_id) ?? [];
    list.push(candidate);
    bySegment.set(candidate.segment_id, list);
  }
  for (const list of bySegment.values()) {
    list.sort((a, b) => a.start_char - b.start_char);
    for (let i = 1; i < list.length; i++) {
      if (list[i].start_char < list[i - 1].end_char) return true;
    }
  }
  return false;
}
