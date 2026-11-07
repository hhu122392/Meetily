import { invoke } from '@tauri-apps/api/core';

export type TemplateSection = {
  id: string;
  title: string;
  instruction: string;
  format: 'paragraph' | 'list' | 'string';
  item_format: string | null;
  example_item_format: string | null;
  required: boolean;
  empty_behavior: 'omit' | 'show_not_mentioned';
};
export type TemplateDocument = {
  schema_version: 2; id: string; name: string; description: string; version: number;
  locale: string | null; tags: string[]; source: Record<string, unknown>;
  created_at: string; updated_at: string; sections: TemplateSection[];
  extensions: Record<string, unknown>;
};
export type TemplateEntry = Pick<TemplateDocument, 'id' | 'name' | 'description' | 'locale' | 'version'> & { readonly: boolean };
export type TemplateLibrary = { templates: TemplateEntry[]; defaultTemplateId: string; warnings: string[] };
export const TEMPLATES_CHANGED = 'meetily:templates-changed';
export const MAX_TEMPLATE_BYTES = 800_000;

export function newSection(): TemplateSection {
  return { id: `section_${crypto.randomUUID().replaceAll('-', '')}`, title: '', instruction: '', format: 'paragraph', item_format: null, example_item_format: null, required: true, empty_behavior: 'show_not_mentioned' };
}
export function newTemplate(): TemplateDocument {
  const now = new Date().toISOString();
  return { schema_version: 2, id: `custom_${crypto.randomUUID().replaceAll('-', '')}`, name: '', description: '', version: 1, locale: 'zh-CN', tags: [], source: { type: 'manual' }, created_at: now, updated_at: now, sections: [newSection()], extensions: {} };
}
export function validateTemplateDraft(doc: TemplateDocument): string | null {
  if (!doc.name.trim() || [...doc.name].length > 120) return '请填写模板名称，最多 120 字';
  if (!doc.description.trim() || [...doc.description].length > 1000) return '请填写模板说明，最多 1000 字';
  if (!doc.sections.length || doc.sections.length > 50) return '模板需要 1 至 50 个章节';
  const titles = new Set<string>();
  for (const [index, section] of doc.sections.entries()) {
    if (!section.title.trim() || [...section.title].length > 120) return `第 ${index + 1} 章需要标题，最多 120 字`;
    if (titles.has(section.title.trim())) return '章节标题不能重复';
    titles.add(section.title.trim());
    if (!section.instruction.trim() || [...section.instruction].length > 10000) return `请填写第 ${index + 1} 章的生成要求，最多 10000 字`;
  }
  return null;
}
export const templateLibrary = {
  list: () => invoke<TemplateLibrary>('template_library_list'),
  read: (id: string) => invoke<TemplateDocument>('template_library_read', { templateId: id }),
  export: (id: string) => invoke<string | null>('template_library_export', { templateId: id }),
  save: (document: TemplateDocument, expectedVersion: number | null) => invoke<TemplateDocument>('template_library_save', { document, expectedVersion }),
  duplicate: (id: string) => invoke<TemplateDocument>('template_library_duplicate', { templateId: id }),
  import: (templateJson: string) => invoke<TemplateDocument>('template_library_import', { templateJson }),
  remove: (id: string) => invoke<void>('template_library_delete', { templateId: id }),
  setDefault: (id: string) => invoke<void>('template_library_set_default', { templateId: id }),
};
