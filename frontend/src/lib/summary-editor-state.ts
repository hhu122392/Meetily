/** Compare saved content, including formatting, without editor-generated block ids. */
export function summaryBlocksFingerprint(blocks: readonly unknown[]): string {
  const canonical = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(canonical);
    if (value && typeof value === 'object') {
      return Object.fromEntries(Object.entries(value).sort(([a], [b]) => a.localeCompare(b))
        .map(([key, child]) => [key, canonical(child)]));
    }
    return value;
  };
  const blockContent = (block: unknown): unknown => {
    if (!block || typeof block !== 'object' || Array.isArray(block)) return block;
    const { id: _id, children, ...content } = block as Record<string, unknown>;
    return { ...content, children: Array.isArray(children) ? children.map(blockContent) : [] };
  };
  return JSON.stringify(canonical(blocks.map(blockContent)));
}
