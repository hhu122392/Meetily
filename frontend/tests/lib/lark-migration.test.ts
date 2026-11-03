import { describe, expect, test } from 'bun:test';
import { Database } from 'bun:sqlite';
import { readFileSync } from 'node:fs';

describe('Lark storage migration', () => {
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
