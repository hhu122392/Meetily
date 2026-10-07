import assert from 'node:assert/strict';
import test from 'node:test';
import i18next from 'i18next';
import { resources } from '../../src/i18n/resources';

test('every action field and review hint uses the actual Chinese and English resources', async () => {
  const instance = i18next.createInstance();
  await instance.init({ resources, lng: 'zh-CN', fallbackLng: 'en', defaultNS: 'summary' });
  for (const lng of ['zh-CN', 'en']) {
    await instance.changeLanguage(lng);
    for (const field of ['owner', 'time', 'acceptance', 'status', 'dependency', 'blocker'] as const) {
      const key = `evidence.fields.${field}` as const;
      assert.ok(instance.exists(key, { lng, fallbackLng: false }));
      assert.notEqual(instance.t(key, { ns: 'summary' }), key);
    }
    assert.notEqual(instance.t('evidence.related', { ns: 'summary' }), 'evidence.related');
  }
  const copy = structuredClone(resources);
  delete (copy['zh-CN'].summary.evidence.fields as Partial<typeof copy['zh-CN']['summary']['evidence']['fields']>).status;
  const fallback = i18next.createInstance();
  await fallback.init({ resources: copy, lng: 'zh-CN', fallbackLng: 'en', defaultNS: 'summary' });
  assert.equal(fallback.t('evidence.fields.status', { ns: 'summary' }), resources.en.summary.evidence.fields.status);
});
