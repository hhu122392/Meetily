import { describe, expect, test } from 'bun:test';
import { Database } from 'bun:sqlite';
import { readFileSync } from 'node:fs';

describe('Lark storage migration', () => {
  test('本地会议记录随会议删除，项目目录按应用隔离', () => {
    const db = new Database(':memory:');
    try {
      db.exec('PRAGMA foreign_keys = ON; CREATE TABLE meetings (id TEXT PRIMARY KEY);');
      db.exec(readFileSync(new URL('../../src-tauri/migrations/20261114000000_local_meeting_delivery.sql', import.meta.url), 'utf8'));
      db.exec("INSERT INTO meetings VALUES ('a');");
      db.exec("INSERT INTO lark_manual_drafts(id,meeting_id,app_id,config_revision,source_revision,payload,created_at) VALUES('draft','a','app1','config','source','{}',1)");
      expect(db.query('SELECT status FROM lark_manual_drafts').get()).toEqual({ status: 'preview' });
      db.exec("INSERT INTO lark_document_archives(fingerprint,app_id,meeting_id,status) VALUES('key','app1','a','creating')");
      expect(() => db.exec("INSERT INTO lark_document_archives(fingerprint,app_id,meeting_id,status) VALUES('key','app1','a','creating')")).toThrow();
      db.exec("INSERT INTO meeting_processing_timings(meeting_id,transcription_seconds) VALUES('a',12.5)");
      db.exec("INSERT INTO lark_archive_projects VALUES('app1','战略会','url1'),('app2','战略会','url2')");
      db.exec("INSERT INTO meeting_template_people VALUES('standard_meeting','甲、乙','甲')");
      expect(db.query('SELECT parent_url FROM lark_archive_projects WHERE app_id=?').get('app1')).toEqual({ parent_url: 'url1' });
      db.exec("DELETE FROM meetings WHERE id='a'");
      for (const table of ['lark_manual_drafts', 'lark_document_archives', 'meeting_processing_timings']) {
        expect(db.query(`SELECT * FROM ${table}`).all()).toHaveLength(0);
      }
      expect(db.query('SELECT * FROM lark_archive_projects').all()).toHaveLength(2);
      expect(db.query('SELECT * FROM meeting_template_people').all()).toHaveLength(1);
    } finally { db.close(); }
  });
  test('enforces singleton settings, unique delivery receipts and meeting deletion cleanup', () => {
    const db = new Database(':memory:');
    try {
      db.exec('PRAGMA foreign_keys = ON; CREATE TABLE meetings (id TEXT PRIMARY KEY);');
      db.exec(readFileSync(new URL('../../src-tauri/migrations/20261014000000_add_lark_bot.sql', import.meta.url), 'utf8'));
      db.exec("INSERT INTO meetings VALUES ('a'); INSERT INTO lark_bot_settings VALUES (1, '{}');");
      expect(() => db.exec("INSERT INTO lark_bot_settings VALUES (2, '{}')")).toThrow();
      db.exec("INSERT INTO lark_summary_deliveries VALUES ('fingerprint-a', 'a', 'now')");
      expect(() => db.exec("INSERT INTO lark_summary_deliveries VALUES ('fingerprint-a', 'a', 'later')")).toThrow();
      db.exec("DELETE FROM meetings WHERE id = 'a'");
      expect(db.query('SELECT * FROM lark_summary_deliveries').all()).toHaveLength(0);
      expect(db.query('SELECT * FROM lark_bot_settings').all()).toHaveLength(1);
    } finally { db.close(); }
  });
});
