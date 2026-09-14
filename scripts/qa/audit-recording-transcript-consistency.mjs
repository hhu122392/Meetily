import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const [sqlite, database, meetingId, folder] = process.argv.slice(2);
assert(sqlite && database && folder && /^meeting-[0-9a-f-]+$/i.test(meetingId ?? ''),
  'Usage: node audit-recording-transcript-consistency.mjs <sqlite.exe> <database> <meeting-id> <recording-folder>');
const query = sql => JSON.parse(execFileSync(sqlite, ['-readonly', '-json', database, sql],
  { encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 }).trim() || '[]');
const meetings = query(`SELECT folder_path FROM meetings WHERE id='${meetingId}'`);
assert.equal(meetings.length, 1, 'Expected exactly one meeting');
assert.equal(path.resolve(meetings[0].folder_path).toLowerCase(), path.resolve(folder).toLowerCase(),
  'The database meeting and recording folder must refer to the same recording');
const document = JSON.parse(fs.readFileSync(path.join(folder, 'transcripts.json'), 'utf8'));
assert(Array.isArray(document.segments), 'Missing file segments');
assert.equal(document.total_segments, document.segments.length, 'Invalid file segment count');
const stored = query(`SELECT transcript AS text,audio_start_time,audio_end_time,duration
  FROM transcripts WHERE meeting_id='${meetingId}' ORDER BY audio_start_time`);
const signature = segment => {
  assert.equal(typeof segment.text, 'string');
  return JSON.stringify([segment.text, ...['audio_start_time', 'audio_end_time', 'duration'].map(key => {
    assert(Number.isFinite(segment[key]), `Invalid ${key}`);
    return Math.round(segment[key] * 1e6);
  })]);
};
const canonical = segments => segments.map(signature).sort();
// This check must distinguish a missing final sentence, not merely valid JSON.
assert.notDeepEqual(canonical([{ text: '末句', audio_start_time: 1, audio_end_time: 2, duration: 1 }]), canonical([]));
const fileRows = canonical(document.segments);
const databaseRows = canonical(stored);
const consistent = fileRows.length === databaseRows.length
  && fileRows.every((row, index) => row === databaseRows[index]);
console.log(JSON.stringify({ checkedAt: new Date().toISOString(), meetingId,
  folder: path.resolve(folder), fileCount: fileRows.length, databaseCount: databaseRows.length,
  consistent, fileSegments: document.segments, databaseSegments: stored,
  note: 'Compared exact text and timestamps rounded to microseconds; legacy IDs differ between stores.' }, null, 2));
process.exitCode = consistent ? 0 : 1;
