import { expect, test } from 'bun:test';
import { downloadErrorMessage } from '../../src/lib/download-error';

test('preserves the string rejection returned by Tauri', () => {
  expect(downloadErrorMessage('连接被重置 (os error 10054)')).toBe('连接被重置 (os error 10054)');
  expect(downloadErrorMessage(new Error('磁盘空间不足'))).toBe('磁盘空间不足');
  expect(downloadErrorMessage(null)).toContain('下载失败');
});
