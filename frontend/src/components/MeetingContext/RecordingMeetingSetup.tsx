'use client';

import { useEffect, useState } from 'react';
import { ChevronDown, LockKeyhole, Plus, Settings2, Trash2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import { HelpHint } from '@/components/ui/help-hint';
import { VisuallyHidden } from '@/components/ui/visually-hidden';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { createMeetingContextId, splitAliases } from '@/lib/meeting-context';
import type { RecordingMeetingSetupState } from '@/hooks/useRecordingMeetingSetup';
import type { RecordingAttendanceStatus } from '@/types/summary-template';

function optionalValue(value: string): string | null {
  const trimmed = value.trim();
  return trimmed || null;
}

function AliasDraftInput({
  aliases,
  placeholder,
  disabled,
  onCommit,
}: {
  aliases: string[];
  placeholder: string;
  disabled: boolean;
  onCommit: (aliases: string[]) => void;
}) {
  const serialized = aliases.join('，');
  const [value, setValue] = useState(serialized);
  useEffect(() => setValue(serialized), [serialized]);
  const commit = () => {
    const parsed = splitAliases(value);
    setValue(parsed.join('，'));
    onCommit(parsed);
  };
  return (
    <Input
      value={value}
      disabled={disabled}
      placeholder={placeholder}
      onChange={(event) => setValue(event.target.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === 'Enter') {
          event.preventDefault();
          commit();
        }
      }}
    />
  );
}

export function RecordingMeetingSetup({
  setup,
  isRecording,
}: {
  setup: RecordingMeetingSetupState;
  isRecording: boolean;
}) {
  const { t } = useTranslation('templates');
  const [open, setOpen] = useState(false);
  const [peopleQuery, setPeopleQuery] = useState('');
  const enabledPeople = setup.profile?.people.filter((person) => person.enabled) ?? [];
  const normalizedPeopleQuery = peopleQuery.trim().toLowerCase();
  const visiblePeople = normalizedPeopleQuery
    ? enabledPeople.filter((person) => (
        [person.display_name, person.department, person.role]
          .filter(Boolean)
          .join(' ')
          .toLowerCase()
          .includes(normalizedPeopleQuery)
      ))
    : enabledPeople;
  const candidateCount = enabledPeople.length + (setup.draft?.guests.length ?? 0);
  const templateName = setup.details?.template.name ?? t('recordingSetup.noTemplate');

  const attendanceFor = (personId: string): RecordingAttendanceStatus => (
    setup.draft?.attendance.find((item) => item.personId === personId)?.attendance ?? 'attending'
  );

  // 几十人的会议不可能逐个点下拉：先给一眼能看懂的计数，再给三个批量按钮
  const attendanceCounts = { attending: 0, absent: 0, expected: 0 };
  for (const person of enabledPeople) {
    const status = attendanceFor(person.person_id);
    if (status === 'attending' || status === 'absent' || status === 'expected') {
      attendanceCounts[status] += 1;
    }
  }

  const setAllAttendance = (status: 'attending' | 'absent' | 'expected') => {
    setup.updateDraft((draft) => ({
      ...draft,
      hostPersonId: status === 'absent' ? null : draft.hostPersonId,
      attendance: draft.attendance.map((item) => ({ ...item, attendance: status })),
    }));
  };

  const setAttendance = (personId: string, attendance: Exclude<RecordingAttendanceStatus, 'guest'>) => {
    setup.updateDraft((draft) => ({
      ...draft,
      hostPersonId: attendance === 'absent' && draft.hostPersonId === personId
        ? null
        : draft.hostPersonId,
      attendance: draft.attendance.map((item) => (
        item.personId === personId ? { ...item, attendance } : item
      )),
    }));
  };

  return (
    <>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="max-w-[620px] rounded-full bg-white shadow-sm"
        onClick={() => setOpen(true)}
      >
        {isRecording ? <LockKeyhole aria-hidden="true" /> : <Settings2 aria-hidden="true" />}
        <span className="truncate">
          {isRecording
            ? t('recordingSetup.locked', { template: templateName })
            : t('recordingSetup.summary', { template: templateName, count: candidateCount })}
        </span>
        {/* 这是一个可点的入口，不是状态标签：给个箭头提示能展开 */}
        {!isRecording && <ChevronDown className="h-4 w-4 flex-none text-gray-400" aria-hidden="true" />}
      </Button>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-h-[88vh] max-w-[calc(100vw-2rem)] overflow-y-auto sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle className="flex items-center gap-1.5">
              {t('recordingSetup.title')}
              <HelpHint
                label={t('recordingSetup.titleHelpLabel')}
                text={isRecording ? t('recordingSetup.lockedDescription') : t('recordingSetup.description')}
              />
            </DialogTitle>
            {/* 说明收进"?"气泡，但保留给读屏的无障碍描述 */}
            <VisuallyHidden>
              <DialogDescription>
                {isRecording ? t('recordingSetup.lockedDescription') : t('recordingSetup.description')}
              </DialogDescription>
            </VisuallyHidden>
          </DialogHeader>

          {setup.error && (
            <p className="rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-800" role="alert">
              {t('recordingSetup.loadWarning')}
            </p>
          )}

          <label className="block text-sm font-medium text-gray-800">
            <span>{t('recordingSetup.template')}</span>
            <Select
              value={setup.details?.template.id ?? ''}
              disabled={isRecording || setup.isLoading}
              onValueChange={(value) => void setup.selectTemplate(value)}
            >
              <SelectTrigger className="mt-2"><SelectValue placeholder={t('recordingSetup.selectTemplate')} /></SelectTrigger>
              <SelectContent>
                {setup.templates.map((template) => (
                  <SelectItem key={`${template.origin}:${template.id}`} value={template.id}>{template.name}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </label>

          {!setup.profile || !setup.draft ? (
            <p className="rounded-md bg-gray-50 p-4 text-sm text-gray-600">
              {setup.isLoading ? t('recordingSetup.loading') : t('recordingSetup.noPreset')}
            </p>
          ) : (
            <div className="space-y-6">
              {setup.profile.fixed_meeting_mechanism && (
                <div className="flex items-center gap-1 text-sm text-gray-600">
                  <span className="font-medium">{t('recordingSetup.mechanism')}：</span>
                  <HelpHint text={setup.profile.fixed_meeting_mechanism} label={t('recordingSetup.mechanism')} />
                </div>
              )}

              <section>
                <h3 className="flex items-center gap-1.5 font-semibold text-gray-900">
                  {t('recordingSetup.attendanceTitle')}
                  <HelpHint
                    label={t('recordingSetup.attendanceHelpLabel')}
                    text={t('recordingSetup.attendanceHelp')}
                  />
                </h3>
                {enabledPeople.length > 0 && (
                  <div className="mt-3 flex flex-wrap items-center gap-2">
                    <span className="text-xs text-gray-500" role="status" aria-live="polite">
                      {t('recordingSetup.attendanceSummary', {
                        attending: attendanceCounts.attending,
                        absent: attendanceCounts.absent,
                        expected: attendanceCounts.expected,
                      })}
                    </span>
                    <div
                      className="ml-auto flex flex-wrap items-center gap-1"
                      role="group"
                      aria-label={t('recordingSetup.attendanceBulkLabel')}
                    >
                      <Button type="button" variant="ghost" size="sm" disabled={isRecording} onClick={() => setAllAttendance('attending')}>
                        {t('recordingSetup.markAllAttending')}
                      </Button>
                      <Button type="button" variant="ghost" size="sm" disabled={isRecording} onClick={() => setAllAttendance('absent')}>
                        {t('recordingSetup.markAllAbsent')}
                      </Button>
                      <Button type="button" variant="ghost" size="sm" disabled={isRecording} onClick={() => setAllAttendance('expected')}>
                        {t('recordingSetup.markAllExpected')}
                      </Button>
                    </div>
                  </div>
                )}
                {/* 人一多就得能找：8 人以上才显示筛选框，几个人的会议不添乱 */}
                {enabledPeople.length >= 8 && (
                  <Input
                    className="mt-3"
                    value={peopleQuery}
                    onChange={(event) => setPeopleQuery(event.target.value)}
                    placeholder={t('recordingSetup.filterPeople')}
                    aria-label={t('recordingSetup.filterPeople')}
                  />
                )}
                <div className="mt-3 space-y-2">
                  {visiblePeople.map((person) => (
                    <div key={person.person_id} className="grid items-center gap-3 rounded-md border border-gray-200 p-3 sm:grid-cols-[1fr_180px]">
                      <div className="min-w-0">
                        <p className="truncate text-sm font-medium text-gray-900">{person.display_name}</p>
                        {(person.department || person.role) && (
                          <p className="truncate text-xs text-gray-500">{[person.department, person.role].filter(Boolean).join(' · ')}</p>
                        )}
                      </div>
                      <Select
                        value={attendanceFor(person.person_id)}
                        disabled={isRecording}
                        onValueChange={(value: 'expected' | 'attending' | 'absent') => setAttendance(person.person_id, value)}
                      >
                        <SelectTrigger><SelectValue /></SelectTrigger>
                        <SelectContent>
                          <SelectItem value="expected">{t('recordingSetup.expected')}</SelectItem>
                          <SelectItem value="attending">{t('recordingSetup.attending')}</SelectItem>
                          <SelectItem value="absent">{t('recordingSetup.absent')}</SelectItem>
                        </SelectContent>
                      </Select>
                    </div>
                  ))}
                  {enabledPeople.length > 0 && visiblePeople.length === 0 && (
                    <p className="rounded-md border border-gray-200 bg-gray-50 p-3 text-sm text-gray-500">
                      {t('recordingSetup.filterNoMatch')}
                    </p>
                  )}
                </div>
              </section>

              <label className="block text-sm font-medium text-gray-800">
                <span>{t('recordingSetup.host')}</span>
                <Select
                  value={setup.draft.hostPersonId ?? 'none'}
                  disabled={isRecording}
                  onValueChange={(value) => setup.updateDraft((draft) => ({
                    ...draft,
                    hostPersonId: value === 'none' ? null : value,
                    attendance: draft.attendance.map((item) => (
                      item.personId === value ? { ...item, attendance: 'attending' } : item
                    )),
                  }))}
                >
                  <SelectTrigger className="mt-2"><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem value="none">{t('recordingSetup.hostUnspecified')}</SelectItem>
                    {enabledPeople
                      .filter((person) => attendanceFor(person.person_id) !== 'absent')
                      .map((person) => <SelectItem key={person.person_id} value={person.person_id}>{person.display_name}</SelectItem>)}
                    {setup.draft.guests.map((guest) => <SelectItem key={guest.personId} value={guest.personId}>{guest.displayName || t('recordingSetup.unnamedGuest')}</SelectItem>)}
                  </SelectContent>
                </Select>
              </label>

              <section className="border-t border-gray-100 pt-5">
                <div className="flex items-center justify-between gap-3">
                  <div>
                    <h3 className="flex items-center gap-1 font-semibold text-gray-900">{t('recordingSetup.guestsTitle')}<HelpHint text={t('recordingSetup.guestsHelp')} /></h3>
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    disabled={isRecording}
                    onClick={() => setup.updateDraft((draft) => ({
                      ...draft,
                      guests: [...draft.guests, {
                        personId: createMeetingContextId('person'),
                        displayName: '',
                        aliases: [],
                        department: null,
                        role: null,
                      }],
                    }))}
                  >
                    <Plus aria-hidden="true" />{t('recordingSetup.addGuest')}
                  </Button>
                </div>
                <div className="mt-3 space-y-3">
                  {setup.draft.guests.map((guest, index) => (
                    <div key={guest.personId} className="rounded-md border border-gray-200 p-3">
                      <div className="grid gap-2 sm:grid-cols-2">
                        <Input
                          value={guest.displayName}
                          disabled={isRecording}
                          placeholder={t('recordingSetup.guestName', { number: index + 1 })}
                          onChange={(event) => setup.updateDraft((draft) => ({
                            ...draft,
                            guests: draft.guests.map((item) => item.personId === guest.personId
                              ? { ...item, displayName: event.target.value }
                              : item),
                          }))}
                        />
                        <AliasDraftInput
                          aliases={guest.aliases}
                          disabled={isRecording}
                          placeholder={t('recordingSetup.aliasesPlaceholder')}
                          onCommit={(aliases) => setup.updateDraft((draft) => ({
                            ...draft,
                            guests: draft.guests.map((item) => item.personId === guest.personId ? { ...item, aliases } : item),
                          }))}
                        />
                        <Input
                          value={guest.department ?? ''}
                          disabled={isRecording}
                          placeholder={t('editor.meetingContext.people.department')}
                          onChange={(event) => setup.updateDraft((draft) => ({
                            ...draft,
                            guests: draft.guests.map((item) => item.personId === guest.personId
                              ? { ...item, department: optionalValue(event.target.value) }
                              : item),
                          }))}
                        />
                        <div className="flex gap-2">
                          <Input
                            value={guest.role ?? ''}
                            disabled={isRecording}
                            placeholder={t('editor.meetingContext.people.role')}
                            onChange={(event) => setup.updateDraft((draft) => ({
                              ...draft,
                              guests: draft.guests.map((item) => item.personId === guest.personId
                                ? { ...item, role: optionalValue(event.target.value) }
                                : item),
                            }))}
                          />
                          <Button
                            type="button"
                            size="icon"
                            variant="ghost"
                            disabled={isRecording}
                            aria-label={t('recordingSetup.removeGuest', { number: index + 1 })}
                            onClick={() => setup.updateDraft((draft) => ({
                              ...draft,
                              hostPersonId: draft.hostPersonId === guest.personId ? null : draft.hostPersonId,
                              guests: draft.guests.filter((item) => item.personId !== guest.personId),
                            }))}
                          ><Trash2 className="h-4 w-4 text-red-600" aria-hidden="true" /></Button>
                        </div>
                      </div>
                    </div>
                  ))}
                </div>
              </section>

              <section className="border-t border-gray-100 pt-5">
                <div className="flex items-center justify-between gap-3">
                  <div>
                    <h3 className="flex items-center gap-1 font-semibold text-gray-900">{t('recordingSetup.termsTitle')}<HelpHint text={t('recordingSetup.termsHelp')} /></h3>
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    disabled={isRecording}
                    onClick={() => setup.updateDraft((draft) => ({
                      ...draft,
                      additionalTerms: [...draft.additionalTerms, {
                        termId: createMeetingContextId('term'),
                        canonical: '',
                        aliases: [],
                        category: null,
                      }],
                    }))}
                  ><Plus aria-hidden="true" />{t('recordingSetup.addTerm')}</Button>
                </div>
                <div className="mt-3 space-y-3">
                  {setup.draft.additionalTerms.map((term, index) => (
                    <div key={term.termId} className="grid gap-2 rounded-md border border-gray-200 p-3 sm:grid-cols-[1fr_1fr_auto]">
                      <Input
                        value={term.canonical}
                        disabled={isRecording}
                        placeholder={t('recordingSetup.termName', { number: index + 1 })}
                        onChange={(event) => setup.updateDraft((draft) => ({
                          ...draft,
                          additionalTerms: draft.additionalTerms.map((item) => item.termId === term.termId
                            ? { ...item, canonical: event.target.value }
                            : item),
                        }))}
                      />
                      <AliasDraftInput
                        aliases={term.aliases}
                        disabled={isRecording}
                        placeholder={t('recordingSetup.aliasesPlaceholder')}
                        onCommit={(aliases) => setup.updateDraft((draft) => ({
                          ...draft,
                          additionalTerms: draft.additionalTerms.map((item) => item.termId === term.termId ? { ...item, aliases } : item),
                        }))}
                      />
                      <Button
                        type="button"
                        size="icon"
                        variant="ghost"
                        disabled={isRecording}
                        aria-label={t('recordingSetup.removeTerm', { number: index + 1 })}
                        onClick={() => setup.updateDraft((draft) => ({
                          ...draft,
                          additionalTerms: draft.additionalTerms.filter((item) => item.termId !== term.termId),
                        }))}
                      ><Trash2 className="h-4 w-4 text-red-600" aria-hidden="true" /></Button>
                    </div>
                  ))}
                </div>
              </section>
            </div>
          )}

          <DialogFooter>
            {!isRecording && setup.isCustomized && (
              <Button type="button" variant="outline" onClick={() => void setup.resetAdjustments()}>
                {t('recordingSetup.reset')}
              </Button>
            )}
            <Button type="button" onClick={() => setOpen(false)}>{t('recordingSetup.done')}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
