import 'fake-indexeddb/auto';
import assert from 'node:assert/strict';
import test from 'node:test';
import { IndexedDBService } from '../../src/services/indexedDBService';

const row = (sequence_id: number, revision: number, text: string, is_partial = true) => ({
  sequence_id, revision, text, is_partial, timestamp: '12:00:00', confidence: .85,
  audio_start_time: sequence_id * 20, audio_end_time: sequence_id * 20 + 20, duration: 20,
});
let next = 0;
const database = () => new IndexedDBService(`Meetily-boundary-test-${next++}`);
const metadata = (meetingId: string) => ({ meetingId, title: 'Test', startTime: 0, lastUpdated: 0, transcriptCount: 0, savedToSQLite: false });

test('tail and next row save together; late partial cannot restore an old version', async () => {
  const db = database();
  await db.saveMeetingMetadata(metadata('a'));
  await db.saveTranscripts('a', [row(0, 1, '自20世纪。')]);
  await db.saveTranscripts('a', [row(0, 2, '完整前句。', false), row(1, 1, '自20世纪60年代以来。')]);
  await db.saveTranscripts('a', [row(0, 1, '旧半句')]);
  const rows = await db.getTranscripts('a');
  assert.equal(rows.length, 2);
  assert.deepEqual(rows.map(r => r.sequenceId), [0, 1]);
  assert.deepEqual(rows.map(r => r.text), ['完整前句。', '自20世纪60年代以来。']);
  assert.equal(rows[0].is_partial, false);
  assert.equal((await db.getMeetingMetadata('a'))?.transcriptCount, 2);
});

test('concurrent saves preserve the highest revision and distinct repeated speech', async () => {
  const db = database();
  await Promise.all([
    db.saveTranscripts('a', [row(0, 1, '旧')]),
    db.saveTranscripts('a', [row(0, 3, '可以可以。', false), row(1, 1, '可以可以。')]),
    db.saveTranscripts('a', [row(0, 2, '不该覆盖')]),
  ]);
  const rows = await db.getTranscripts('a');
  assert.equal(rows.length, 2);
  assert.deepEqual(rows.map(r => r.text), ['可以可以。', '可以可以。']);
});

test('same sequence numbers in different meetings do not overwrite each other', async () => {
  const db = database();
  await db.saveTranscripts('a', [row(0, 1, '第一场')]);
  await db.saveTranscripts('b', [row(0, 1, '第二场')]);
  assert.equal((await db.getTranscripts('a'))[0].text, '第一场');
  assert.equal((await db.getTranscripts('b'))[0].text, '第二场');
});

test('an invalid member aborts the whole batch', async () => {
  const db = database();
  await db.saveTranscripts('a', [row(0, 1, '原文')]);
  await assert.rejects(db.saveTranscripts('a', [row(0, 2, '修订'), { ...row(1, 1, '无效'), sequence_id: NaN }]));
  assert.equal((await db.getTranscripts('a'))[0].text, '原文');
});

test('a storage failure after an earlier put rolls back the entire revision batch', async () => {
  const db = database();
  await db.saveMeetingMetadata(metadata('a'));
  await db.saveTranscripts('a', [row(0, 1, '原文')]);
  await assert.rejects(db.saveTranscripts('a', [row(0, 2, '修订'), { ...row(1, 1, '故障'), invalid: () => 1 }]));
  const rows = await db.getTranscripts('a');
  assert.deepEqual(rows.map(r => r.text), ['原文']);
  assert.equal((await db.getMeetingMetadata('a'))?.transcriptCount, 1);
});

test('stop flush waits for writes started before database initialization finishes', async () => {
  const db = database();
  const pending = db.saveTranscripts('a', [row(0, 2, '最后一句', false)]);
  await db.flushTranscriptWrites();
  assert.equal((await db.getTranscripts('a'))[0].text, '最后一句');
  await pending;
});
