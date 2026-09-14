import type { SavedTranscript, Transcript } from '../types';

export function completedSnapshotRows(rows: SavedTranscript[]): Transcript[] {
  const ids = new Set<number>();
  return rows.map(row => {
    if (row.is_partial || !Number.isSafeInteger(row.sequence_id) || ids.has(row.sequence_id)) {
      throw new Error('Final transcript contains unfinished or duplicate rows');
    }
    ids.add(row.sequence_id);
    return { ...row, timestamp: row.display_time, chunk_start_time: row.audio_start_time };
  });
}

export function isNewerTranscript(previous: { revision?: number; is_partial?: boolean }, incoming: { revision?: number; is_partial?: boolean }): boolean {
  const oldRevision = previous.revision ?? 0;
  const newRevision = incoming.revision ?? 0;
  if (newRevision !== oldRevision) return newRevision > oldRevision;
  // Legacy providers do not have revisions. A final row must still reject a
  // late partial event; versioned rows reject duplicate delivery entirely.
  return newRevision === 0 && previous.is_partial === true;
}

export function mergeLiveTranscripts(previous: Transcript[], incoming: Transcript[]): Transcript[] {
  const key = (row: Transcript) => row.sequence_id !== undefined ? `seq:${row.sequence_id}` : `id:${row.id}`;
  const rows = new Map(previous.map(row => [key(row), row]));
  for (const row of incoming) {
    const old = rows.get(key(row));
    if (!old) rows.set(key(row), row);
    else if (isNewerTranscript(old, row)) rows.set(key(row), { ...old, ...row, id: old.id });
  }
  return [...rows.values()].sort((a, b) =>
    (a.chunk_start_time ?? a.audio_start_time ?? 0) - (b.chunk_start_time ?? b.audio_start_time ?? 0)
    || (a.sequence_id ?? 0) - (b.sequence_id ?? 0));
}
