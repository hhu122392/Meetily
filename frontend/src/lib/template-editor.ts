import type {
  EmptyBehavior,
  TemplateFieldIssue,
  TemplateFormat,
  TemplateSectionV2,
  TemplateV2,
} from '@/types/summary-template';
import {
  meetingContextProfileFromExtensions,
  validateMeetingContextProfile,
} from '@/lib/meeting-context';
import { MEETING_CONTEXT_EXTENSION_KEY } from '@/types/summary-template';

export interface TemplateEditorSection extends TemplateSectionV2 {
  draftKey: string;
}

export interface TemplateEditorDraft extends Omit<TemplateV2, 'sections'> {
  sections: TemplateEditorSection[];
}

export interface LocalTemplateIssue extends TemplateFieldIssue {
  messageKey:
    | 'templates.editor.validation.id'
    | 'templates.editor.validation.name'
    | 'templates.editor.validation.description'
    | 'templates.editor.validation.locale'
    | 'templates.editor.validation.tags'
    | 'templates.editor.validation.sections'
    | 'templates.editor.validation.sectionId'
    | 'templates.editor.validation.sectionIdDuplicate'
    | 'templates.editor.validation.sectionTitle'
    | 'templates.editor.validation.sectionInstruction'
    | 'templates.editor.validation.meetingContext';
}

let fallbackDraftKeySequence = 0;

export function createDraftKey(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  fallbackDraftKeySequence += 1;
  return `draft-${Date.now()}-${fallbackDraftKeySequence}`;
}

export function createTemplateDraft(now = new Date().toISOString()): TemplateEditorDraft {
  return {
    schemaVersion: 2,
    id: 'new_meeting_template',
    name: '',
    description: '',
    version: 1,
    locale: 'zh-CN',
    tags: [],
    source: {
      type: 'manual',
      originalFileName: null,
      originalFileSha256: null,
      importedAt: null,
      copiedFromTemplateId: null,
    },
    createdAt: now,
    updatedAt: now,
    sections: [{
      draftKey: createDraftKey(),
      id: 'summary',
      title: '',
      instruction: '',
      format: 'paragraph',
      itemFormat: null,
      exampleItemFormat: null,
      required: true,
      emptyBehavior: 'show_not_mentioned',
    }],
    extensions: {},
  };
}

export function templateToEditorDraft(template: TemplateV2): TemplateEditorDraft {
  return {
    ...structuredClone(template),
    sections: template.sections.map((section) => ({
      ...structuredClone(section),
      draftKey: createDraftKey(),
    })),
  };
}

export function editorDraftToTemplate(draft: TemplateEditorDraft): TemplateV2 {
  return {
    ...structuredClone(draft),
    sections: draft.sections.map(({ draftKey: _draftKey, ...section }) => section),
  };
}

function safeSuffix(value: string): string {
  const suffix = value.toLocaleLowerCase().replace(/[^a-z0-9]/g, '').slice(0, 12);
  return suffix || 'new';
}

export function slugifyTemplateId(name: string, suffix = 'new'): string {
  const slug = name
    .normalize('NFKD')
    .toLocaleLowerCase()
    .replace(/[^a-z0-9_-]+/g, '_')
    .replace(/_+/g, '_')
    .replace(/^[_-]+|[_-]+$/g, '')
    .slice(0, 80)
    .replace(/[_-]+$/g, '');

  if (/^[a-z0-9][a-z0-9_-]*[a-z0-9]$/.test(slug) && slug.length >= 3) return slug;
  return `template_${safeSuffix(suffix)}`;
}

export function slugifySectionId(title: string, suffix = 'section'): string {
  const slug = title
    .normalize('NFKD')
    .toLocaleLowerCase()
    .replace(/[^a-z0-9_-]+/g, '_')
    .replace(/_+/g, '_')
    .replace(/^[_-]+/g, '')
    .slice(0, 80);
  if (/^[a-z0-9][a-z0-9_-]*$/.test(slug)) return slug;
  return `section_${safeSuffix(suffix)}`;
}

function uniqueSectionId(draft: TemplateEditorDraft, preferred: string): string {
  const used = new Set(draft.sections.map((section) => section.id));
  if (!used.has(preferred)) return preferred;
  for (let index = 2; index < 10_000; index += 1) {
    const candidate = `${preferred}_${index}`.slice(0, 80);
    if (!used.has(candidate)) return candidate;
  }
  return `section_${Date.now()}`;
}

export function addTemplateSection(draft: TemplateEditorDraft): TemplateEditorDraft {
  if (draft.sections.length >= 50) return draft;
  const id = uniqueSectionId(draft, 'new_section');
  return {
    ...draft,
    sections: [...draft.sections, {
      draftKey: createDraftKey(),
      id,
      title: '',
      instruction: '',
      format: 'paragraph',
      itemFormat: null,
      exampleItemFormat: null,
      required: false,
      emptyBehavior: 'omit',
    }],
  };
}

export function updateTemplateSection(
  draft: TemplateEditorDraft,
  draftKey: string,
  changes: Partial<Omit<TemplateEditorSection, 'draftKey'>>,
): TemplateEditorDraft {
  return {
    ...draft,
    sections: draft.sections.map((section) =>
      section.draftKey === draftKey ? { ...section, ...changes } : section),
  };
}

export function duplicateTemplateSection(
  draft: TemplateEditorDraft,
  draftKey: string,
): TemplateEditorDraft {
  const index = draft.sections.findIndex((section) => section.draftKey === draftKey);
  if (index < 0 || draft.sections.length >= 50) return draft;
  const source = draft.sections[index];
  const copy: TemplateEditorSection = {
    ...structuredClone(source),
    draftKey: createDraftKey(),
    id: uniqueSectionId(draft, `${source.id}_copy`.slice(0, 80)),
  };
  const sections = [...draft.sections];
  sections.splice(index + 1, 0, copy);
  return { ...draft, sections };
}

export function removeTemplateSection(
  draft: TemplateEditorDraft,
  draftKey: string,
): TemplateEditorDraft {
  if (draft.sections.length <= 1) return draft;
  return {
    ...draft,
    sections: draft.sections.filter((section) => section.draftKey !== draftKey),
  };
}

export function moveTemplateSection(
  draft: TemplateEditorDraft,
  draftKey: string,
  destinationIndex: number,
): TemplateEditorDraft {
  const sourceIndex = draft.sections.findIndex((section) => section.draftKey === draftKey);
  if (sourceIndex < 0) return draft;
  const boundedDestination = Math.max(0, Math.min(destinationIndex, draft.sections.length - 1));
  if (sourceIndex === boundedDestination) return draft;

  const sections = [...draft.sections];
  const [section] = sections.splice(sourceIndex, 1);
  sections.splice(boundedDestination, 0, section);
  return { ...draft, sections };
}

export function changeSectionFormat(
  draft: TemplateEditorDraft,
  draftKey: string,
  format: TemplateFormat,
): TemplateEditorDraft {
  return updateTemplateSection(draft, draftKey, {
    format,
    itemFormat: format === 'list' ? draft.sections.find((section) => section.draftKey === draftKey)?.itemFormat ?? '- {{item}}' : null,
    exampleItemFormat: format === 'list' ? draft.sections.find((section) => section.draftKey === draftKey)?.exampleItemFormat ?? null : null,
  });
}

export function changeSectionEmptyBehavior(
  draft: TemplateEditorDraft,
  draftKey: string,
  emptyBehavior: EmptyBehavior,
): TemplateEditorDraft {
  return updateTemplateSection(draft, draftKey, { emptyBehavior });
}

export function validateTemplateDraft(draft: TemplateEditorDraft): LocalTemplateIssue[] {
  const issues: LocalTemplateIssue[] = [];
  const add = (path: string, messageKey: LocalTemplateIssue['messageKey']) => {
    issues.push({ code: 'EDITOR_VALIDATION', path, messageKey });
  };

  if (!/^[a-z0-9][a-z0-9_-]*[a-z0-9]$/.test(draft.id) || draft.id.length < 3 || draft.id.length > 80) {
    add('/id', 'templates.editor.validation.id');
  }
  if (!draft.name.trim() || draft.name.length > 120) add('/name', 'templates.editor.validation.name');
  if (!draft.description.trim() || draft.description.length > 1000) {
    add('/description', 'templates.editor.validation.description');
  }
  if (draft.locale && !/^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})*$/.test(draft.locale)) {
    add('/locale', 'templates.editor.validation.locale');
  }
  if (draft.tags.length > 20 || new Set(draft.tags).size !== draft.tags.length || draft.tags.some((tag) => !tag.trim() || tag.length > 40)) {
    add('/tags', 'templates.editor.validation.tags');
  }
  if (draft.sections.length < 1 || draft.sections.length > 50) {
    add('/sections', 'templates.editor.validation.sections');
  }

  const ids = new Set<string>();
  draft.sections.forEach((section, index) => {
    if (!/^[a-z0-9][a-z0-9_-]*$/.test(section.id) || section.id.length > 80) {
      add(`/sections/${index}/id`, 'templates.editor.validation.sectionId');
    } else if (ids.has(section.id)) {
      add(`/sections/${index}/id`, 'templates.editor.validation.sectionIdDuplicate');
    }
    ids.add(section.id);
    if (!section.title.trim() || section.title.length > 120) {
      add(`/sections/${index}/title`, 'templates.editor.validation.sectionTitle');
    }
    if (!section.instruction.trim() || section.instruction.length > 10_000) {
      add(`/sections/${index}/instruction`, 'templates.editor.validation.sectionInstruction');
    }
  });

  if (MEETING_CONTEXT_EXTENSION_KEY in draft.extensions) {
    const profile = meetingContextProfileFromExtensions(draft.extensions);
    if (!profile) {
      add(
        `/extensions/${MEETING_CONTEXT_EXTENSION_KEY}`,
        'templates.editor.validation.meetingContext',
      );
    } else {
      validateMeetingContextProfile(profile).forEach((issue) => {
        add(
          `/extensions/${MEETING_CONTEXT_EXTENSION_KEY}${issue.path}`,
          'templates.editor.validation.meetingContext',
        );
      });
    }
  }
  return issues;
}

function canonicalize(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, child]) => [key, canonicalize(child)]),
    );
  }
  return value;
}

export function editableTemplateFingerprint(draft: TemplateEditorDraft): string {
  const template = editorDraftToTemplate(draft);
  const editable = {
    id: template.id,
    name: template.name,
    description: template.description,
    locale: template.locale,
    tags: template.tags,
    source: template.source,
    sections: template.sections,
    extensions: template.extensions,
  };
  return JSON.stringify(canonicalize(editable));
}

export function isTemplateDraftDirty(
  draft: TemplateEditorDraft,
  baseline: TemplateEditorDraft,
): boolean {
  return editableTemplateFingerprint(draft) !== editableTemplateFingerprint(baseline);
}

export function issueForPath(
  issues: readonly TemplateFieldIssue[],
  path: string,
): TemplateFieldIssue | undefined {
  return issues.find((issue) => issue.path === path);
}

export function parseEditorJson(text: string): { value: unknown | null; error: string | null } {
  try {
    return { value: JSON.parse(text), error: null };
  } catch (error) {
    return {
      value: null,
      error: error instanceof Error ? error.message : 'Invalid JSON',
    };
  }
}

export function formatEditorJson(draft: TemplateEditorDraft): string {
  return `${JSON.stringify(editorDraftToTemplate(draft), null, 2)}\n`;
}

export function createSaveAsCopyDraft(
  draft: TemplateEditorDraft,
  id: string,
  name: string,
  now = new Date().toISOString(),
): TemplateEditorDraft {
  return {
    ...structuredClone(draft),
    id,
    name,
    version: 1,
    source: {
      type: 'manual',
      originalFileName: null,
      originalFileSha256: null,
      importedAt: null,
      copiedFromTemplateId: null,
    },
    createdAt: now,
    updatedAt: now,
    sections: draft.sections.map((section) => ({ ...structuredClone(section) })),
  };
}
