import assert from 'node:assert/strict';
import test from 'node:test';
import React from 'react';
import TestRenderer, { act } from 'react-test-renderer';
import { renderToStaticMarkup } from 'react-dom/server';
import { useTranscriptStreaming } from '../../src/hooks/useTranscriptStreaming';
import { TranscriptView } from '../../src/components/TranscriptView';
import { VirtualizedTranscriptView } from '../../src/components/VirtualizedTranscriptView';
import { TooltipProvider } from '../../src/components/ui/tooltip';
import type { TranscriptSegmentData } from '../../src/types';
import { i18n } from '../../src/i18n';

Object.assign(globalThis, { React });
void i18n.changeLanguage('zh-CN');

test('a versioned transcript shows the full current text and its same-id revision immediately', t => {
  const View = ({ row }: { row: TranscriptSegmentData }) => {
    const streaming = useTranscriptStreaming([row], true, true);
    return <span>{streaming.getDisplayText(row)}</span>;
  };
  const original = { id: '0', timestamp: 0, text: '这是第一版尚未说完的句子', revision: 1, is_partial: true };
  let renderer: TestRenderer.ReactTestRenderer;
  act(() => { renderer = TestRenderer.create(<View row={original} />); });
  t.after(() => act(() => renderer.unmount()));
  assert.equal(renderer!.root.findByType('span').children.join(''), original.text);
  const next = { ...original, text: '这是同一句的完整修订。', revision: 2, is_partial: false };
  act(() => renderer.update(<View row={next} />));
  assert.equal(renderer!.root.findByType('span').children.join(''), next.text);
});

test('both transcript views preserve repeated words and fillers verbatim', () => {
  const text = 'I I I, um, this is is is the exact text. 对对对，可以可以。';
  const regular = renderToStaticMarkup(<TooltipProvider><TranscriptView transcripts={[{
    id: '0', sequence_id: 0, timestamp: '12:00:00', text,
  }]} /></TooltipProvider>);
  const virtual = renderToStaticMarkup(<TooltipProvider><VirtualizedTranscriptView segments={[{
    id: '0', timestamp: 0, text,
  }]} /></TooltipProvider>);
  assert.ok(regular.includes(text), 'regular view must retain the original words');
  assert.ok(virtual.includes(text), 'virtual view must retain the original words');
});
