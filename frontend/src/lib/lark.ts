import type { MeetingSummary } from '@/types';

export interface LarkConfig {
  webhook: string;
  secret: string;
  target_name: string;
}
export interface LarkConfigView {
  settings: LarkConfig;
  revision: string;
}
export interface LarkSendResult {
  already_sent: boolean;
  sent_at: string;
}

// Only legacy summaries are converted here. Modern summaries must be exported
// from the live editor so unsaved edits are never replaced by an older snapshot.
export function legacySummaryText(summary: MeetingSummary): string {
  if ('markdown' in summary || 'summary_json' in summary) return '';
  const order = Array.isArray(summary._section_order)
    ? summary._section_order.filter((key): key is string => typeof key === 'string')
    : [];
  const keys = [...new Set([...order, ...Object.keys(summary)])];
  return keys.filter(key => !['MeetingName', '_section_order'].includes(key)).map(key => {
    const section: unknown = summary[key];
    if (!section || typeof section !== 'object' || !('title' in section) || !('blocks' in section)
      || typeof section.title !== 'string' || !Array.isArray(section.blocks)) return '';
    const lines = section.blocks.map((block: unknown) =>
      block && typeof block === 'object' && 'content' in block && typeof block.content === 'string'
        ? `- ${block.content}` : '').filter(Boolean);
    return lines.length ? `## ${section.title}\n\n${lines.join('\n')}` : '';
  }).filter(Boolean).join('\n\n');
}

export function larkMessageText(title: string, date: string, summary: string): string {
  return `会议纪要：${title.trim()}\n会议时间：${date.trim()}\n\n${summary.trim()}`;
}
