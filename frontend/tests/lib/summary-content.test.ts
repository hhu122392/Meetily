import { describe, expect, test } from 'bun:test';
import { hasVisibleSummaryContent, parseSummaryContent, readSummaryMetadata } from '../../src/lib/summary-content';

describe('summary content validation', () => {
  test('accepts visible markdown and rejects whitespace or reasoning markers', () => {
    expect(hasVisibleSummaryContent({ markdown: '# Summary\nVisible' })).toBe(true);
    expect(hasVisibleSummaryContent({ markdown: '   ' })).toBe(false);
    expect(hasVisibleSummaryContent({ markdown: '<think>private</think>' })).toBe(false);
    expect(hasVisibleSummaryContent({
      markdown: '<thinking class="internal">private</thinking>',
    })).toBe(false);
    expect(hasVisibleSummaryContent({ markdown: '<thinker>Visible</thinker>' })).toBe(true);
  });

  test('validates BlockNote and legacy summary content', () => {
    expect(hasVisibleSummaryContent({ summary_json: [] })).toBe(false);
    expect(hasVisibleSummaryContent({
      summary_json: [{ content: [{ type: 'text', text: 'Visible' }] }],
    })).toBe(true);
    expect(hasVisibleSummaryContent({
      Decisions: { blocks: [{ content: '   ' }] },
    })).toBe(false);
    expect(hasVisibleSummaryContent({
      Decisions: { blocks: [{ content: 'Ship it' }] },
    })).toBe(true);
  });

  test('parses one historical double-encoded payload', () => {
    expect(parseSummaryContent('{"markdown":"Visible"}')).toEqual({ markdown: 'Visible' });
    expect(parseSummaryContent('{not json')).toBeNull();
  });
});

test("行动项提取失败仍保留可见纪要和独立警告", () => {
  const value = { markdown: "# 例会\n正文", meeting_action_items: [], meeting_action_items_error: "提取失败，请重试" };
  expect(parseSummaryContent(value)).toEqual(value);
  expect(readSummaryMetadata(value).actionItemsError).toBe("提取失败，请重试");
  expect(readSummaryMetadata({ markdown: "正文" }).actionItemsError).toBeNull();
});
