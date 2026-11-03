import { expect, test } from 'bun:test';
import { companyStorageKey, initializeCompanyScope, storageKeyForCompany } from '../../src/lib/company-scope';

test('原有恢复记录仅归原公司，新公司使用独立缓存，身份未加载时拒绝读取', () => {
  expect(() => companyStorageKey('MeetilyRecoveryDB')).toThrow();
  expect(storageKeyForCompany('legacy', 'MeetilyRecoveryDB')).toBe('MeetilyRecoveryDB');
  expect(storageKeyForCompany('company-a', 'MeetilyRecoveryDB')).not.toBe(storageKeyForCompany('company-b', 'MeetilyRecoveryDB'));
  expect(storageKeyForCompany('company-a', 'transcription_progress')).not.toBe('transcription_progress');
  initializeCompanyScope('company-a');
  expect(companyStorageKey('MeetilyRecoveryDB')).toBe('MeetilyRecoveryDB:company:company-a');
  expect(() => initializeCompanyScope('company-b')).toThrow('重启');
});
