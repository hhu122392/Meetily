import { beforeEach, expect, test } from 'bun:test';
import { getSettingsReturnPath, rememberSettingsReturnPath } from '../../src/lib/settings-navigation';

beforeEach(() => rememberSettingsReturnPath('/'));

test('会议进入设置后反复切换模板库，返回仍保留会议及查询参数', () => {
  const meeting = '/meeting-details?id=meeting-123#transcript';
  rememberSettingsReturnPath(meeting);
  for (const path of ['/settings', '/settings/templates', '/settings', '/settings/templates', '/settings']) {
    rememberSettingsReturnPath(path);
  }
  expect(getSettingsReturnPath()).toBe(meeting);
});

test('下一次从首页进入设置，不沿用上一次会议', () => {
  rememberSettingsReturnPath('/meeting-details?id=previous');
  rememberSettingsReturnPath('/');
  rememberSettingsReturnPath('/settings/templates');
  expect(getSettingsReturnPath()).toBe('/');
});

test('直接打开设置或模板库，没有来源时返回首页', () => {
  rememberSettingsReturnPath('/settings/templates');
  rememberSettingsReturnPath('/settings?tab=summary');
  expect(getSettingsReturnPath()).toBe('/');
});
