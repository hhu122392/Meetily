import { afterEach, describe, expect, test } from 'bun:test';
import { renderToStaticMarkup } from 'react-dom/server';
import { EmptyStateSummary } from '../../src/components/EmptyStateSummary';
import { setUiLanguage } from '../../src/i18n';

afterEach(() => setUiLanguage('zh-CN'));

describe('empty summary state', () => {
  test.each(['zh-CN', 'en'] as const)('renders a persistent retryable error in %s', (language) => {
    setUiLanguage(language);
    const html = renderToStaticMarkup(
      <EmptyStateSummary
        error="Summary request failed"
        hasModel
        onGenerate={() => {}}
      />,
    );

    expect(html).toContain('role="alert"');
    expect(html).toContain('Summary request failed');
    expect(html).toContain(language === 'zh-CN' ? '重新生成纪要' : 'Retry summary');
  });
});
