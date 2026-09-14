'use client';

import { useState, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import {
  ArrowDown,
  ArrowUp,
  ChevronDown,
  ChevronsDown,
  ChevronsUp,
  Copy,
  Plus,
  Trash2,
  X,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { Switch } from '@/components/ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import type { TemplateFieldIssue, TemplateFormat } from '@/types/summary-template';
import {
  addTemplateSection,
  changeSectionEmptyBehavior,
  changeSectionFormat,
  duplicateTemplateSection,
  moveTemplateSection,
  removeTemplateSection,
  updateTemplateSection,
  type TemplateEditorDraft,
  type TemplateEditorSection,
} from '@/lib/template-editor';
import { templateTranslationKey } from '@/lib/template-library';
import { MeetingContextProfileEditor } from '@/components/templates/MeetingContextProfileEditor';

export function TemplateEditorFields({
  draft,
  setDraft,
  issues,
  isCreate,
  onNameChange,
  onIdChange,
}: {
  draft: TemplateEditorDraft;
  setDraft: React.Dispatch<React.SetStateAction<TemplateEditorDraft>>;
  issues: readonly TemplateFieldIssue[];
  isCreate: boolean;
  onNameChange: (name: string) => void;
  onIdChange: (id: string) => void;
}) {
  const { t } = useTranslation('templates');
  const [tagInput, setTagInput] = useState('');
  const [expandedKey, setExpandedKey] = useState<string | null>(draft.sections[0]?.draftKey ?? null);
  const [deleteSection, setDeleteSection] = useState<TemplateEditorSection | null>(null);

  const issueAt = (path: string) => issues.find((issue) => issue.path === path);
  const renderIssue = (path: string) => {
    const issue = issueAt(path);
    return issue ? (
      <p className="mt-1 text-xs text-red-600" role="alert">
        {t(templateTranslationKey(issue.messageKey), issue.params ?? {})}
      </p>
    ) : null;
  };

  const addTag = () => {
    const tag = tagInput.trim();
    if (!tag || tag.length > 40 || draft.tags.length >= 20 || draft.tags.includes(tag)) return;
    setDraft((current) => ({ ...current, tags: [...current.tags, tag] }));
    setTagInput('');
  };

  const handleTagKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Enter' || event.key === ',') {
      event.preventDefault();
      addTag();
    } else if (event.key === 'Backspace' && !tagInput && draft.tags.length > 0) {
      setDraft((current) => ({ ...current, tags: current.tags.slice(0, -1) }));
    }
  };

  const handleAddSection = () => {
    setDraft((current) => {
      const next = addTemplateSection(current);
      setExpandedKey(next.sections[next.sections.length - 1].draftKey);
      return next;
    });
  };

  const confirmDelete = () => {
    if (!deleteSection) return;
    setDraft((current) => removeTemplateSection(current, deleteSection.draftKey));
    setDeleteSection(null);
  };

  return (
    <div className="space-y-6">
      <section className="rounded-lg border border-gray-200 bg-white p-5 shadow-sm">
        <div className="grid gap-5 sm:grid-cols-2">
          <label className="block text-sm font-medium text-gray-800">
            <span>{t('editor.fields.name')}</span>
            <Input
              value={draft.name}
              maxLength={120}
              onChange={(event) => onNameChange(event.target.value)}
              placeholder={t('editor.fields.namePlaceholder')}
              aria-invalid={!!issueAt('/name')}
              className="mt-2"
            />
            {renderIssue('/name')}
          </label>

          <label className="block text-sm font-medium text-gray-800">
            <span>{t('editor.fields.id')}</span>
            <Input
              value={draft.id}
              maxLength={80}
              readOnly={!isCreate}
              onChange={(event) => onIdChange(event.target.value.toLocaleLowerCase())}
              aria-invalid={!!issueAt('/id')}
              className="mt-2 font-mono"
            />
            <p className="mt-1 text-xs text-gray-500">{t('editor.fields.idHelp')}</p>
            {renderIssue('/id')}
          </label>
        </div>

        <label className="mt-5 block text-sm font-medium text-gray-800">
          <span>{t('editor.fields.description')}</span>
          <Textarea
            value={draft.description}
            maxLength={1000}
            rows={3}
            onChange={(event) => setDraft((current) => ({ ...current, description: event.target.value }))}
            placeholder={t('editor.fields.descriptionPlaceholder')}
            aria-invalid={!!issueAt('/description')}
            className="mt-2 resize-y"
          />
          <div className="mt-1 flex justify-between gap-3 text-xs text-gray-400">
            <div>{renderIssue('/description')}</div>
            <span>{draft.description.length}/1000</span>
          </div>
        </label>

        <div className="mt-5 grid gap-5 sm:grid-cols-2">
          <label className="block text-sm font-medium text-gray-800">
            <span>{t('editor.fields.locale')}</span>
            <Select
              value={draft.locale ?? 'auto'}
              onValueChange={(value) => setDraft((current) => ({ ...current, locale: value === 'auto' ? null : value }))}
            >
              <SelectTrigger className="mt-2">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="auto">{t('editor.fields.localeAuto')}</SelectItem>
                <SelectItem value="zh-CN">{t('editor.fields.localeChinese')}</SelectItem>
                <SelectItem value="en">{t('editor.fields.localeEnglish')}</SelectItem>
              </SelectContent>
            </Select>
            {renderIssue('/locale')}
          </label>

          <div className="text-sm font-medium text-gray-800">
            <span>{t('editor.fields.tags')}</span>
            <div className="mt-2 rounded-md border border-gray-200 bg-white p-2 focus-within:ring-1 focus-within:ring-gray-900">
              <div className="flex flex-wrap gap-1.5">
                {draft.tags.map((tag) => (
                  <span key={tag} className="inline-flex items-center gap-1 rounded-full bg-blue-50 px-2 py-1 text-xs font-medium text-blue-700">
                    {tag}
                    <button
                      type="button"
                      aria-label={t('editor.fields.removeTag', { tag })}
                      onClick={() => setDraft((current) => ({ ...current, tags: current.tags.filter((value) => value !== tag) }))}
                      className="rounded-full hover:text-blue-950 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
                    >
                      <X className="h-3 w-3" aria-hidden="true" />
                    </button>
                  </span>
                ))}
                <input
                  value={tagInput}
                  maxLength={40}
                  onChange={(event) => setTagInput(event.target.value)}
                  onKeyDown={handleTagKeyDown}
                  onBlur={addTag}
                  placeholder={draft.tags.length ? '' : t('editor.fields.tagsPlaceholder')}
                  className="min-w-40 flex-1 border-0 bg-transparent px-1 py-1 text-sm font-normal outline-none"
                />
              </div>
            </div>
            {renderIssue('/tags')}
          </div>
        </div>
      </section>

      <MeetingContextProfileEditor draft={draft} setDraft={setDraft} />

      <section aria-labelledby="template-sections-title">
        <div className="mb-3 flex flex-wrap items-end justify-between gap-3">
          <div>
            <h2 id="template-sections-title" className="text-lg font-semibold text-gray-900">{t('editor.sections.title')}</h2>
            <p className="mt-1 text-sm text-gray-600">{t('editor.sections.description')}</p>
          </div>
          <Button type="button" variant="outline" onClick={handleAddSection} disabled={draft.sections.length >= 50}>
            <Plus aria-hidden="true" />
            {t('editor.sections.add')}
          </Button>
        </div>
        {draft.sections.length >= 50 && <p className="mb-3 text-sm text-amber-700">{t('editor.sections.limit')}</p>}
        {renderIssue('/sections')}

        <div className="space-y-3">
          {draft.sections.map((section, index) => {
            const expanded = expandedKey === section.draftKey;
            const label = section.title.trim() || t('editor.sections.section', { number: index + 1 });
            const basePath = `/sections/${index}`;
            const sectionHasError = issues.some((issue) => issue.path.startsWith(`${basePath}/`));
            return (
              <article key={section.draftKey} className={`rounded-lg border bg-white shadow-sm ${sectionHasError ? 'border-red-300' : 'border-gray-200'}`}>
                <div className="flex min-w-0 items-center gap-2 p-3">
                  <button
                    type="button"
                    aria-expanded={expanded}
                    aria-label={t(expanded ? 'editor.sections.collapse' : 'editor.sections.expand', { title: label })}
                    onClick={() => setExpandedKey(expanded ? null : section.draftKey)}
                    className="flex min-w-0 flex-1 items-center gap-3 rounded-md p-1 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
                  >
                    <ChevronDown className={`h-4 w-4 shrink-0 transition-transform ${expanded ? 'rotate-180' : ''}`} aria-hidden="true" />
                    <span className="truncate font-semibold">{index + 1}. {label}</span>
                    <span className="shrink-0 rounded-full bg-gray-100 px-2 py-0.5 text-xs text-gray-500">{section.format}</span>
                  </button>
                  <div className="flex shrink-0 items-center gap-0.5" onClick={(event) => event.stopPropagation()}>
                    <IconButton label={t('editor.sections.moveTop')} disabled={index === 0} onClick={() => setDraft((current) => moveTemplateSection(current, section.draftKey, 0))}><ChevronsUp /></IconButton>
                    <IconButton label={t('editor.sections.moveUp')} disabled={index === 0} onClick={() => setDraft((current) => moveTemplateSection(current, section.draftKey, index - 1))}><ArrowUp /></IconButton>
                    <IconButton label={t('editor.sections.moveDown')} disabled={index === draft.sections.length - 1} onClick={() => setDraft((current) => moveTemplateSection(current, section.draftKey, index + 1))}><ArrowDown /></IconButton>
                    <IconButton label={t('editor.sections.moveBottom')} disabled={index === draft.sections.length - 1} onClick={() => setDraft((current) => moveTemplateSection(current, section.draftKey, current.sections.length - 1))}><ChevronsDown /></IconButton>
                    <IconButton label={t('editor.sections.duplicate')} disabled={draft.sections.length >= 50} onClick={() => setDraft((current) => duplicateTemplateSection(current, section.draftKey))}><Copy /></IconButton>
                    <IconButton label={t('editor.sections.delete')} disabled={draft.sections.length <= 1} destructive onClick={() => setDeleteSection(section)}><Trash2 /></IconButton>
                  </div>
                </div>

                {expanded && (
                  <div className="space-y-5 border-t border-gray-100 p-4">
                    <div className="grid gap-4 sm:grid-cols-2">
                      <label className="text-sm font-medium text-gray-800">
                        <span>{t('editor.sections.sectionTitle')}</span>
                        <Input
                          value={section.title}
                          maxLength={120}
                          onChange={(event) => setDraft((current) => updateTemplateSection(current, section.draftKey, { title: event.target.value }))}
                          placeholder={t('editor.sections.sectionTitlePlaceholder')}
                          aria-invalid={!!issueAt(`${basePath}/title`)}
                          className="mt-2"
                        />
                        {renderIssue(`${basePath}/title`)}
                      </label>
                      <label className="text-sm font-medium text-gray-800">
                        <span>{t('editor.sections.id')}</span>
                        <Input
                          value={section.id}
                          maxLength={80}
                          onChange={(event) => setDraft((current) => updateTemplateSection(current, section.draftKey, { id: event.target.value.toLocaleLowerCase() }))}
                          aria-invalid={!!issueAt(`${basePath}/id`)}
                          className="mt-2 font-mono"
                        />
                        {renderIssue(`${basePath}/id`)}
                      </label>
                    </div>

                    <label className="block text-sm font-medium text-gray-800">
                      <span>{t('editor.sections.instruction')}</span>
                      <Textarea
                        value={section.instruction}
                        maxLength={10_000}
                        rows={4}
                        onChange={(event) => setDraft((current) => updateTemplateSection(current, section.draftKey, { instruction: event.target.value }))}
                        placeholder={t('editor.sections.instructionPlaceholder')}
                        aria-invalid={!!issueAt(`${basePath}/instruction`)}
                        className="mt-2 resize-y"
                      />
                      <div className="mt-1 flex justify-between gap-2 text-xs text-gray-400">
                        <div>{renderIssue(`${basePath}/instruction`)}</div>
                        <span>{section.instruction.length}/10000</span>
                      </div>
                    </label>

                    <div className="grid gap-4 sm:grid-cols-2">
                      <label className="text-sm font-medium text-gray-800">
                        <span>{t('editor.sections.format')}</span>
                        <Select value={section.format} onValueChange={(value: TemplateFormat) => setDraft((current) => changeSectionFormat(current, section.draftKey, value))}>
                          <SelectTrigger className="mt-2"><SelectValue /></SelectTrigger>
                          <SelectContent>
                            <SelectItem value="paragraph">{t('editor.sections.formatParagraph')}</SelectItem>
                            <SelectItem value="list">{t('editor.sections.formatList')}</SelectItem>
                            <SelectItem value="string">{t('editor.sections.formatString')}</SelectItem>
                          </SelectContent>
                        </Select>
                      </label>
                      <label className="text-sm font-medium text-gray-800">
                        <span>{t('editor.sections.emptyBehavior')}</span>
                        <Select value={section.emptyBehavior} onValueChange={(value: 'omit' | 'show_not_mentioned') => setDraft((current) => changeSectionEmptyBehavior(current, section.draftKey, value))}>
                          <SelectTrigger className="mt-2"><SelectValue /></SelectTrigger>
                          <SelectContent>
                            <SelectItem value="omit">{t('editor.sections.emptyOmit')}</SelectItem>
                            <SelectItem value="show_not_mentioned">{t('editor.sections.emptyShow')}</SelectItem>
                          </SelectContent>
                        </Select>
                      </label>
                    </div>

                    {section.format === 'list' && (
                      <div className="grid gap-4 sm:grid-cols-2">
                        <label className="text-sm font-medium text-gray-800">
                          <span>{t('editor.sections.itemFormat')}</span>
                          <Textarea
                            value={section.itemFormat ?? ''}
                            maxLength={5000}
                            rows={3}
                            onChange={(event) => setDraft((current) => updateTemplateSection(current, section.draftKey, { itemFormat: event.target.value || null }))}
                            placeholder={t('editor.sections.itemFormatPlaceholder', { owner: '{{owner}}', action: '{{action}}', deadline: '{{deadline}}' })}
                            className="mt-2 resize-y font-mono text-xs"
                          />
                        </label>
                        <label className="text-sm font-medium text-gray-800">
                          <span>{t('editor.sections.exampleItemFormat')}</span>
                          <Textarea
                            value={section.exampleItemFormat ?? ''}
                            maxLength={5000}
                            rows={3}
                            onChange={(event) => setDraft((current) => updateTemplateSection(current, section.draftKey, { exampleItemFormat: event.target.value || null }))}
                            placeholder={t('editor.sections.exampleItemFormatPlaceholder')}
                            className="mt-2 resize-y font-mono text-xs"
                          />
                        </label>
                      </div>
                    )}

                    <div className="flex items-center justify-between rounded-md border border-gray-200 bg-gray-50 p-3">
                      <label htmlFor={`required-${section.draftKey}`} className="text-sm font-medium text-gray-800">{t('editor.sections.required')}</label>
                      <Switch
                        id={`required-${section.draftKey}`}
                        checked={section.required}
                        onCheckedChange={(required) => setDraft((current) => updateTemplateSection(current, section.draftKey, { required }))}
                      />
                    </div>
                  </div>
                )}
              </article>
            );
          })}
        </div>
      </section>

      <Dialog open={deleteSection !== null} onOpenChange={(open) => !open && setDeleteSection(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('editor.sections.deleteTitle', { title: deleteSection?.title || deleteSection?.id || '' })}</DialogTitle>
            <DialogDescription>{t('editor.sections.deleteDescription')}</DialogDescription>
          </DialogHeader>
          {draft.sections.length <= 1 && <p className="text-sm text-red-600" role="alert">{t('editor.sections.cannotDeleteLast')}</p>}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setDeleteSection(null)}>{t('duplicate.cancel')}</Button>
            <Button type="button" variant="destructive" disabled={draft.sections.length <= 1} onClick={confirmDelete}>
              <Trash2 aria-hidden="true" />
              {t('editor.sections.delete')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function IconButton({
  label,
  disabled,
  destructive = false,
  onClick,
  children,
}: {
  label: string;
  disabled: boolean;
  destructive?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      disabled={disabled}
      onClick={(event) => {
        event.preventDefault();
        event.stopPropagation();
        onClick();
      }}
      className={`rounded-md p-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:cursor-not-allowed disabled:opacity-30 ${
        destructive ? 'text-red-500 hover:bg-red-50 hover:text-red-700' : 'text-gray-500 hover:bg-gray-100 hover:text-gray-900'
      }`}
    >
      <span className="[&>svg]:h-4 [&>svg]:w-4">{children}</span>
    </button>
  );
}
