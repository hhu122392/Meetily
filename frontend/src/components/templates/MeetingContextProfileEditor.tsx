'use client';

import { useEffect, useState } from 'react';
import { Plus, Trash2, UsersRound } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Textarea } from '@/components/ui/textarea';
import {
  createEmptyMeetingContextProfile,
  createMeetingContextId,
  meetingContextProfileFromExtensions,
  removeMeetingContextProfileExtension,
  setMeetingContextProfileExtension,
  splitAliases,
  validateMeetingContextProfile,
} from '@/lib/meeting-context';
import type { TemplateEditorDraft } from '@/lib/template-editor';
import type {
  MeetingContextPersonProfile,
  MeetingContextProfile,
  MeetingContextTermProfile,
} from '@/types/summary-template';

interface MeetingContextProfileEditorProps {
  draft: TemplateEditorDraft;
  setDraft: React.Dispatch<React.SetStateAction<TemplateEditorDraft>>;
}

function optionalValue(value: string): string | null {
  const trimmed = value.trim();
  return trimmed || null;
}

function AliasInput({
  aliases,
  placeholder,
  onCommit,
}: {
  aliases: string[];
  placeholder: string;
  onCommit: (aliases: string[]) => void;
}) {
  const serialized = aliases.join('，');
  const [value, setValue] = useState(serialized);

  useEffect(() => {
    setValue(serialized);
  }, [serialized]);

  const commit = () => {
    const parsed = splitAliases(value);
    setValue(parsed.join('，'));
    onCommit(parsed);
  };

  return (
    <Input
      value={value}
      onChange={(event) => setValue(event.target.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === 'Enter') {
          event.preventDefault();
          commit();
        }
      }}
      placeholder={placeholder}
      className="mt-1.5"
    />
  );
}

export function MeetingContextProfileEditor({
  draft,
  setDraft,
}: MeetingContextProfileEditorProps) {
  const { t } = useTranslation('templates');
  const profile = meetingContextProfileFromExtensions(draft.extensions);
  const issues = profile ? validateMeetingContextProfile(profile) : [];

  const updateProfile = (updater: (current: MeetingContextProfile) => MeetingContextProfile) => {
    setDraft((current) => {
      const currentProfile = meetingContextProfileFromExtensions(current.extensions)
        ?? createEmptyMeetingContextProfile();
      return {
        ...current,
        extensions: setMeetingContextProfileExtension(current.extensions, updater(currentProfile)),
      };
    });
  };

  const updatePerson = (personId: string, changes: Partial<MeetingContextPersonProfile>) => {
    updateProfile((current) => ({
      ...current,
      people: current.people.map((person) => (
        person.person_id === personId ? { ...person, ...changes } : person
      )),
    }));
  };

  const updateTerm = (termId: string, changes: Partial<MeetingContextTermProfile>) => {
    updateProfile((current) => ({
      ...current,
      terms: current.terms.map((term) => (
        term.term_id === termId ? { ...term, ...changes } : term
      )),
    }));
  };

  if (!profile) {
    return (
      <section className="rounded-lg border border-dashed border-gray-300 bg-gray-50 p-5" aria-labelledby="meeting-context-title">
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div className="flex min-w-0 items-start gap-3">
            <UsersRound className="mt-0.5 h-5 w-5 shrink-0 text-gray-500" aria-hidden="true" />
            <div>
              <h2 id="meeting-context-title" className="font-semibold text-gray-900">{t('editor.meetingContext.title')}</h2>
              <p className="mt-1 max-w-3xl text-sm text-gray-600">{t('editor.meetingContext.description')}</p>
            </div>
          </div>
          <Button
            type="button"
            variant="outline"
            onClick={() => updateProfile(() => createEmptyMeetingContextProfile())}
          >
            <Plus aria-hidden="true" />
            {t('editor.meetingContext.enable')}
          </Button>
        </div>
      </section>
    );
  }

  return (
    <section className="rounded-lg border border-gray-200 bg-white p-5 shadow-sm" aria-labelledby="meeting-context-title">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <h2 id="meeting-context-title" className="text-lg font-semibold text-gray-900">{t('editor.meetingContext.title')}</h2>
          <p className="mt-1 max-w-3xl text-sm text-gray-600">{t('editor.meetingContext.description')}</p>
        </div>
        <Button
          type="button"
          variant="outline"
          onClick={() => setDraft((current) => ({
            ...current,
            extensions: removeMeetingContextProfileExtension(current.extensions),
          }))}
        >
          <Trash2 aria-hidden="true" />
          {t('editor.meetingContext.removePreset')}
        </Button>
      </div>

      {issues.length > 0 && (
        <div className="mt-4 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-700" role="alert">
          <p className="font-medium">{t('editor.meetingContext.validationSummary', { count: issues.length })}</p>
          <ul className="mt-2 list-disc space-y-1 pl-5">
            {issues.slice(0, 8).map((issue, index) => (
              <li key={`${issue.path}-${issue.code}-${index}`}>
                {t(`editor.meetingContext.validation.${issue.code}`, { defaultValue: issue.code })}
                <span className="ml-1 font-mono text-xs text-red-600">{issue.path}</span>
              </li>
            ))}
          </ul>
        </div>
      )}

      <label className="mt-5 block text-sm font-medium text-gray-800">
        <span>{t('editor.meetingContext.mechanism')}</span>
        <Textarea
          value={profile.fixed_meeting_mechanism ?? ''}
          maxLength={1_000}
          rows={3}
          onChange={(event) => updateProfile((current) => ({
            ...current,
            fixed_meeting_mechanism: optionalValue(event.target.value),
          }))}
          placeholder={t('editor.meetingContext.mechanismPlaceholder')}
          className="mt-2 resize-y"
        />
        <p className="mt-1 text-xs text-gray-500">{t('editor.meetingContext.mechanismHelp')}</p>
      </label>

      <div className="mt-6">
        <div className="flex flex-wrap items-end justify-between gap-3">
          <div>
            <h3 className="font-semibold text-gray-900">{t('editor.meetingContext.people.title')}</h3>
            <p className="mt-1 text-sm text-gray-600">{t('editor.meetingContext.people.description')}</p>
          </div>
          <Button
            type="button"
            variant="outline"
            disabled={profile.people.length >= 100}
            onClick={() => updateProfile((current) => ({
              ...current,
              people: [...current.people, {
                person_id: createMeetingContextId('person'),
                display_name: '',
                aliases: [],
                department: null,
                role: null,
                enabled: true,
              }],
            }))}
          >
            <Plus aria-hidden="true" />
            {t('editor.meetingContext.people.add')}
          </Button>
        </div>

        {profile.people.length === 0 ? (
          <p className="mt-3 rounded-md bg-gray-50 p-4 text-sm text-gray-500">{t('editor.meetingContext.people.empty')}</p>
        ) : (
          <div className="mt-3 space-y-3">
            {profile.people.map((person, index) => (
              <article key={person.person_id} className="rounded-md border border-gray-200 p-4">
                <div className="flex items-center justify-between gap-3">
                  <div className="flex items-center gap-2">
                    <Switch
                      id={`person-enabled-${person.person_id}`}
                      checked={person.enabled}
                      onCheckedChange={(enabled) => updatePerson(person.person_id, { enabled })}
                    />
                    <label htmlFor={`person-enabled-${person.person_id}`} className="text-sm font-semibold text-gray-800">
                      {t('editor.meetingContext.people.item', { number: index + 1 })}
                    </label>
                  </div>
                  <Button
                    type="button"
                    size="icon"
                    variant="ghost"
                    aria-label={t('editor.meetingContext.people.remove', { number: index + 1 })}
                    onClick={() => updateProfile((current) => ({
                      ...current,
                      people: current.people.filter((item) => item.person_id !== person.person_id),
                    }))}
                  >
                    <Trash2 className="h-4 w-4 text-red-600" aria-hidden="true" />
                  </Button>
                </div>
                <div className="mt-3 grid gap-3 sm:grid-cols-2">
                  <label className="text-sm font-medium text-gray-800">
                    <span>{t('editor.meetingContext.people.name')}</span>
                    <Input
                      value={person.display_name}
                      maxLength={80}
                      onChange={(event) => updatePerson(person.person_id, { display_name: event.target.value })}
                      placeholder={t('editor.meetingContext.people.namePlaceholder')}
                      className="mt-1.5"
                    />
                  </label>
                  <label className="text-sm font-medium text-gray-800">
                    <span>{t('editor.meetingContext.aliases')}</span>
                    <AliasInput
                      aliases={person.aliases}
                      onCommit={(aliases) => updatePerson(person.person_id, { aliases })}
                      placeholder={t('editor.meetingContext.people.aliasesPlaceholder')}
                    />
                  </label>
                  <label className="text-sm font-medium text-gray-800">
                    <span>{t('editor.meetingContext.people.department')}</span>
                    <Input
                      value={person.department ?? ''}
                      maxLength={120}
                      onChange={(event) => updatePerson(person.person_id, { department: optionalValue(event.target.value) })}
                      className="mt-1.5"
                    />
                  </label>
                  <label className="text-sm font-medium text-gray-800">
                    <span>{t('editor.meetingContext.people.role')}</span>
                    <Input
                      value={person.role ?? ''}
                      maxLength={120}
                      onChange={(event) => updatePerson(person.person_id, { role: optionalValue(event.target.value) })}
                      className="mt-1.5"
                    />
                  </label>
                </div>
                <p className="mt-2 break-all font-mono text-[11px] text-gray-400">{person.person_id}</p>
              </article>
            ))}
          </div>
        )}
      </div>

      <div className="mt-6 border-t border-gray-100 pt-6">
        <div className="flex flex-wrap items-end justify-between gap-3">
          <div>
            <h3 className="font-semibold text-gray-900">{t('editor.meetingContext.terms.title')}</h3>
            <p className="mt-1 text-sm text-gray-600">{t('editor.meetingContext.terms.description')}</p>
          </div>
          <Button
            type="button"
            variant="outline"
            disabled={profile.terms.length >= 200}
            onClick={() => updateProfile((current) => ({
              ...current,
              terms: [...current.terms, {
                term_id: createMeetingContextId('term'),
                canonical: '',
                aliases: [],
                category: null,
                enabled: true,
              }],
            }))}
          >
            <Plus aria-hidden="true" />
            {t('editor.meetingContext.terms.add')}
          </Button>
        </div>

        {profile.terms.length === 0 ? (
          <p className="mt-3 rounded-md bg-gray-50 p-4 text-sm text-gray-500">{t('editor.meetingContext.terms.empty')}</p>
        ) : (
          <div className="mt-3 space-y-3">
            {profile.terms.map((term, index) => (
              <article key={term.term_id} className="rounded-md border border-gray-200 p-4">
                <div className="flex items-center justify-between gap-3">
                  <div className="flex items-center gap-2">
                    <Switch
                      id={`term-enabled-${term.term_id}`}
                      checked={term.enabled}
                      onCheckedChange={(enabled) => updateTerm(term.term_id, { enabled })}
                    />
                    <label htmlFor={`term-enabled-${term.term_id}`} className="text-sm font-semibold text-gray-800">
                      {t('editor.meetingContext.terms.item', { number: index + 1 })}
                    </label>
                  </div>
                  <Button
                    type="button"
                    size="icon"
                    variant="ghost"
                    aria-label={t('editor.meetingContext.terms.remove', { number: index + 1 })}
                    onClick={() => updateProfile((current) => ({
                      ...current,
                      terms: current.terms.filter((item) => item.term_id !== term.term_id),
                    }))}
                  >
                    <Trash2 className="h-4 w-4 text-red-600" aria-hidden="true" />
                  </Button>
                </div>
                <div className="mt-3 grid gap-3 sm:grid-cols-3">
                  <label className="text-sm font-medium text-gray-800">
                    <span>{t('editor.meetingContext.terms.canonical')}</span>
                    <Input
                      value={term.canonical}
                      maxLength={80}
                      onChange={(event) => updateTerm(term.term_id, { canonical: event.target.value })}
                      placeholder={t('editor.meetingContext.terms.canonicalPlaceholder')}
                      className="mt-1.5"
                    />
                  </label>
                  <label className="text-sm font-medium text-gray-800">
                    <span>{t('editor.meetingContext.aliases')}</span>
                    <AliasInput
                      aliases={term.aliases}
                      onCommit={(aliases) => updateTerm(term.term_id, { aliases })}
                      placeholder={t('editor.meetingContext.terms.aliasesPlaceholder')}
                    />
                  </label>
                  <label className="text-sm font-medium text-gray-800">
                    <span>{t('editor.meetingContext.terms.category')}</span>
                    <Input
                      value={term.category ?? ''}
                      maxLength={120}
                      onChange={(event) => updateTerm(term.term_id, { category: optionalValue(event.target.value) })}
                      placeholder={t('editor.meetingContext.terms.categoryPlaceholder')}
                      className="mt-1.5"
                    />
                  </label>
                </div>
                <p className="mt-2 break-all font-mono text-[11px] text-gray-400">{term.term_id}</p>
              </article>
            ))}
          </div>
        )}
      </div>
    </section>
  );
}
