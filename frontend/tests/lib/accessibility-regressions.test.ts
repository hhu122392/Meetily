import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const sourceRoot = new URL('../../src/', import.meta.url);

async function source(relativePath: string) {
  return readFile(new URL(relativePath, sourceRoot), 'utf8');
}

test('root layout provides a stable document title', async () => {
  const [layout, metadata, constants] = await Promise.all([
    source('app/layout.tsx'),
    source('app/metadata.ts'),
    source('constants/app.ts'),
  ]);
  assert.match(layout, /<title>\{DOCUMENT_TITLE\}<\/title>/);
  assert.match(metadata, /title: DOCUMENT_TITLE/);
  assert.match(constants, /export const DOCUMENT_TITLE = 'Meetily'/);
});

test('main content and settings tabs contain horizontal layout pressure', async () => {
  const [mainContent, settings] = await Promise.all([
    source('components/MainContent/index.tsx'),
    source('app/settings/page.tsx'),
  ]);

  assert.match(mainContent, /min-w-0 flex-1/);
  assert.match(settings, /overflow-x-hidden/);
  assert.match(settings, /overflow-x-auto/);
  assert.match(settings, /shrink-0/);
});

test('template cards do not nest actions inside an interactive card container', async () => {
  const library = await source('components/templates/TemplateLibraryPage.tsx');
  const card = library.slice(library.indexOf('function TemplateCard('), library.indexOf('function PreviewDialog('));

  assert.doesNotMatch(card, /role="button"/);
  assert.doesNotMatch(card, /tabIndex=\{0\}/);
  assert.match(card, /<Button type="button" variant="outline" size="sm"/);
});

test('solid semantic button variants use WCAG-friendly 700-series colors', async () => {
  const button = await source('components/ui/button.tsx');
  assert.match(button, /forced-colors:bg-\[ButtonFace\]/);
  assert.match(button, /forced-colors:text-\[ButtonText\]/);
  assert.match(button, /green: "bg-green-700 text-white hover:bg-green-800"/);
  assert.match(button, /blue: "bg-blue-700 text-white hover:bg-blue-800"/);
  assert.match(button, /red: "bg-red-700 text-white hover:bg-red-800"/);
});
