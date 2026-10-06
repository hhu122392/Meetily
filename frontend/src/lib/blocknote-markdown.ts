import type { Block } from "@blocknote/core";

/** Count whole empty sections, never unknown fields inside useful content. */
export function emptySummarySections(markdown: string) {
  const bodies = markdown.split(/^(?:#{1,6}[ \t]+.+|\*\*[^*\r\n]+\*\*)[ \t\r]*$/m).slice(1);
  const placeholders = bodies.filter(body => {
    const text = body.trim().replace(/^(?:[-*+]\s+|>\s*)/, '').replace(/^\*\*|\*\*$/g, '').trim();
    return /^(?:(?:会议|本节)?未(?:提及|注明|记录)|not mentioned|none noted|not specified)[.。!！]?$/i.test(text);
  }).length;
  const sections = bodies.length;
  return placeholders >= 3 && placeholders * 2 > sections ? { placeholders, sections } : null;
}

interface MarkdownCapableEditor {
  blocksToMarkdownLossy: (blocks: Block[]) => Promise<string>;
}

interface MarkdownConversionOptions {
  source: string;
  fallbackMarkdown?: string;
}

interface MarkdownConversionResult {
  markdown?: string;
  ok: boolean;
}

export async function blocksToMarkdownSafely(
  editor: MarkdownCapableEditor,
  blocks: Block[],
  options: MarkdownConversionOptions,
): Promise<MarkdownConversionResult> {
  try {
    return {
      markdown: await editor.blocksToMarkdownLossy(blocks),
      ok: true,
    };
  } catch (error) {
    console.error("Failed to convert BlockNote blocks to markdown", {
      source: options.source,
      blocksCount: blocks.length,
      error,
    });

    return {
      markdown: options.fallbackMarkdown,
      ok: false,
    };
  }
}
