import assert from 'node:assert/strict';
import { afterEach, describe, test } from 'node:test';
import type { Block } from '@blocknote/core';

import { blocksToMarkdownSafely, emptySummarySections } from '../../src/lib/blocknote-markdown';

const originalConsoleError = console.error;

test('empty summary notice distinguishes unknown fields from whole empty sections', () => {
  // F01 desktop regression: a missing host and two missing blockers triggered
  // "most sections empty" even though both sections had useful facts.
  const report = `**会议信息**

讨论接口回归测试和试点邀请。主持人：会议未提及

**行动计划**

| 行动任务 | 负责人 | 截止时间 | 验收标准 | 当前状态 | 依赖或卡点 |
| --- | --- | --- | --- | --- | --- |
| 完成接口回归测试 | 林州 | 10月9日18点 | 阻断问题为0 | 进行中 | 依赖: 供应商审批通过; 卡点: 会议未提及 |
| 发布试点邀请 | 陈兰 | 10月10日12点 | 20名试点客户全部收到邀请 | 未开始 | 依赖: 接口回归测试通过; 卡点: 会议未提及 |`;
  assert.equal(emptySummarySections(report), null);
  assert.equal(emptySummarySections(report.replaceAll('\n', '\r\n')), null);
  const empty = '**摘要**\n\n会议未提及\n\n## Decisions\n\nNot mentioned.\n\n**讨论**\n\n本节未注明\n\n';
  assert.deepEqual(emptySummarySections(empty + '**行动**\n\n张三整理报告。'), { placeholders: 3, sections: 4 });
  assert.equal(emptySummarySections(empty + '**行动**\n张三整理报告。\n**风险**\n明确风险\n**背景**\n已有背景'), null);
  assert.equal(emptySummarySections('未提及 未提及 未提及'), null);
});

describe('blocksToMarkdownSafely', () => {
  afterEach(() => {
    console.error = originalConsoleError;
  });

  test('returns markdown when conversion succeeds', async () => {
    let conversionCalls = 0;
    const editor = {
      blocksToMarkdownLossy: async () => {
        conversionCalls += 1;
        return '# Summary';
      },
    };

    const result = await blocksToMarkdownSafely(editor, [] as Block[], {
      source: 'test-success',
    });

    assert.deepEqual(result, {
      markdown: '# Summary',
      ok: true,
    });
    assert.equal(conversionCalls, 1);
  });

  test('returns fallback markdown when conversion throws', async () => {
    const error = new Error('conversion failed');
    const editor = {
      blocksToMarkdownLossy: async () => {
        throw error;
      },
    };
    const consoleErrors: unknown[][] = [];
    console.error = (...args: unknown[]) => {
      consoleErrors.push(args);
    };

    const result = await blocksToMarkdownSafely(
      editor,
      [{ id: 'block-1' }] as unknown as Block[],
      {
        source: 'test-fallback',
        fallbackMarkdown: 'existing markdown',
      },
    );

    assert.deepEqual(result, {
      markdown: 'existing markdown',
      ok: false,
    });
    assert.equal(consoleErrors.length, 1);
    assert.deepEqual(consoleErrors[0], [
      'Failed to convert BlockNote blocks to markdown',
      {
        source: 'test-fallback',
        blocksCount: 1,
        error,
      },
    ]);
  });

  test('omits markdown when conversion throws without fallback', async () => {
    const editor = {
      blocksToMarkdownLossy: async () => {
        throw new Error('conversion failed');
      },
    };
    console.error = () => undefined;

    const result = await blocksToMarkdownSafely(editor, [] as Block[], {
      source: 'test-empty-fallback',
    });

    assert.deepEqual(result, {
      markdown: undefined,
      ok: false,
    });
  });
});
