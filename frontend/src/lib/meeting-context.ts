import {
  MEETING_CONTEXT_EXTENSION_KEY,
  type MeetingContextPersonProfile,
  type MeetingContextProfile,
  type MeetingContextProfileIssue,
  type MeetingContextTermProfile,
  type RecordingMeetingContextDraft,
} from '@/types/summary-template';

const MAX_PEOPLE = 100;
const MAX_TERMS = 200;
const MAX_ALIASES = 20;
const MAX_NAME_CHARS = 80;
const MAX_OPTIONAL_CHARS = 120;
const MAX_MECHANISM_CHARS = 1_000;
const FORBIDDEN_CHARACTER = /[\u0000-\u001f\u007f-\u009f\u200e\u200f\u202a-\u202e\u2066-\u2069]/u;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isNullableString(value: unknown): value is string | null {
  return value === null || typeof value === 'string';
}

function isStringArray(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((item) => typeof item === 'string');
}

function parsePerson(value: unknown): MeetingContextPersonProfile | null {
  if (!isRecord(value)
    || typeof value.person_id !== 'string'
    || typeof value.display_name !== 'string'
    || (value.aliases !== undefined && !isStringArray(value.aliases))
    || (value.department !== undefined && !isNullableString(value.department))
    || (value.role !== undefined && !isNullableString(value.role))
    || (value.enabled !== undefined && typeof value.enabled !== 'boolean')) {
    return null;
  }
  return {
    person_id: value.person_id,
    display_name: value.display_name,
    aliases: value.aliases ?? [],
    department: value.department ?? null,
    role: value.role ?? null,
    enabled: value.enabled ?? true,
  };
}

function parseTerm(value: unknown): MeetingContextTermProfile | null {
  if (!isRecord(value)
    || typeof value.term_id !== 'string'
    || typeof value.canonical !== 'string'
    || (value.aliases !== undefined && !isStringArray(value.aliases))
    || (value.category !== undefined && !isNullableString(value.category))
    || (value.enabled !== undefined && typeof value.enabled !== 'boolean')) {
    return null;
  }
  return {
    term_id: value.term_id,
    canonical: value.canonical,
    aliases: value.aliases ?? [],
    category: value.category ?? null,
    enabled: value.enabled ?? true,
  };
}

export function createEmptyMeetingContextProfile(): MeetingContextProfile {
  return {
    schema_version: 1,
    fixed_meeting_mechanism: null,
    people: [],
    terms: [],
  };
}

export function parseMeetingContextProfile(value: unknown): MeetingContextProfile | null {
  if (!isRecord(value)
    || value.schema_version !== 1
    || (value.fixed_meeting_mechanism !== undefined
      && !isNullableString(value.fixed_meeting_mechanism))
    || (value.people !== undefined && !Array.isArray(value.people))
    || (value.terms !== undefined && !Array.isArray(value.terms))) {
    return null;
  }
  const people = (value.people ?? []).map(parsePerson);
  const terms = (value.terms ?? []).map(parseTerm);
  if (people.some((person) => person === null) || terms.some((term) => term === null)) return null;
  return {
    schema_version: 1,
    fixed_meeting_mechanism: value.fixed_meeting_mechanism ?? null,
    people: people as MeetingContextPersonProfile[],
    terms: terms as MeetingContextTermProfile[],
  };
}

export function meetingContextProfileFromExtensions(
  extensions: Record<string, unknown>,
): MeetingContextProfile | null {
  if (!(MEETING_CONTEXT_EXTENSION_KEY in extensions)) return null;
  return parseMeetingContextProfile(extensions[MEETING_CONTEXT_EXTENSION_KEY]);
}

export function meetingContextProfileOrEmpty(
  extensions: Record<string, unknown>,
): MeetingContextProfile {
  return meetingContextProfileFromExtensions(extensions) ?? createEmptyMeetingContextProfile();
}

export function setMeetingContextProfileExtension(
  extensions: Record<string, unknown>,
  profile: MeetingContextProfile,
): Record<string, unknown> {
  return {
    ...structuredClone(extensions),
    [MEETING_CONTEXT_EXTENSION_KEY]: structuredClone(profile),
  };
}

export function removeMeetingContextProfileExtension(
  extensions: Record<string, unknown>,
): Record<string, unknown> {
  const next = structuredClone(extensions);
  delete next[MEETING_CONTEXT_EXTENSION_KEY];
  return next;
}

export function splitAliases(value: string): string[] {
  return value
    .split(/[,，、;；\n]/u)
    .map((alias) => alias.trim())
    .filter(Boolean);
}

function comparisonKey(value: string): string {
  return value.trim().normalize('NFKC').replace(/[A-Z]/g, (character) => character.toLowerCase());
}

function validateRequired(
  value: string,
  path: string,
  maxChars: number,
  issues: MeetingContextProfileIssue[],
): void {
  const trimmed = value.trim();
  if (!trimmed) issues.push({ code: 'EMPTY_VALUE', path });
  if ([...trimmed].length > maxChars) issues.push({ code: 'VALUE_TOO_LONG', path });
  if (FORBIDDEN_CHARACTER.test(value)) issues.push({ code: 'FORBIDDEN_CHARACTER', path });
}

function validateOptional(
  value: string | null,
  path: string,
  maxChars: number,
  issues: MeetingContextProfileIssue[],
): void {
  if (value === null) return;
  if ([...value.trim()].length > maxChars) issues.push({ code: 'VALUE_TOO_LONG', path });
  if (FORBIDDEN_CHARACTER.test(value)) issues.push({ code: 'FORBIDDEN_CHARACTER', path });
}

function validateAliases(
  aliases: string[],
  path: string,
  issues: MeetingContextProfileIssue[],
): void {
  if (aliases.length > MAX_ALIASES) issues.push({ code: 'TOO_MANY_ALIASES', path });
  const seen = new Set<string>();
  aliases.forEach((alias, index) => {
    validateRequired(alias, `${path}/${index}`, MAX_NAME_CHARS, issues);
    const key = comparisonKey(alias);
    if (key && seen.has(key)) issues.push({ code: 'DUPLICATE_ALIAS', path: `${path}/${index}` });
    seen.add(key);
  });
}

function validateUniqueProfiles(
  items: Array<{ id: string; name: string; aliases: string[]; enabled: boolean }>,
  basePath: string,
  idField: string,
  nameField: string,
  codes: { duplicateId: string; duplicateName: string; ambiguousAlias: string; aliasMatchesName: string },
  issues: MeetingContextProfileIssue[],
): void {
  const ids = new Map<string, number>();
  const names = new Map<string, number>();
  const activeNames = new Map<string, number>();
  items.forEach((item, index) => {
    const idKey = comparisonKey(item.id);
    const nameKey = comparisonKey(item.name);
    if (ids.has(idKey)) issues.push({ code: codes.duplicateId, path: `${basePath}/${index}/${idField}` });
    if (names.has(nameKey)) issues.push({ code: codes.duplicateName, path: `${basePath}/${index}/${nameField}` });
    ids.set(idKey, index);
    names.set(nameKey, index);
    if (item.enabled) activeNames.set(nameKey, index);
  });

  const aliases = new Map<string, number>();
  items.forEach((item, index) => {
    if (!item.enabled) return;
    item.aliases.forEach((alias, aliasIndex) => {
      const key = comparisonKey(alias);
      const owner = aliases.get(key);
      if (owner !== undefined && owner !== index) {
        issues.push({ code: codes.ambiguousAlias, path: `${basePath}/${index}/aliases/${aliasIndex}` });
      }
      const nameOwner = activeNames.get(key);
      if (nameOwner !== undefined && nameOwner !== index) {
        issues.push({ code: codes.aliasMatchesName, path: `${basePath}/${index}/aliases/${aliasIndex}` });
      }
      aliases.set(key, index);
    });
  });
}

export function validateMeetingContextProfile(
  profile: MeetingContextProfile,
): MeetingContextProfileIssue[] {
  const issues: MeetingContextProfileIssue[] = [];
  if (profile.schema_version !== 1) issues.push({ code: 'UNSUPPORTED_SCHEMA_VERSION', path: '/schema_version' });
  if (profile.people.length > MAX_PEOPLE) issues.push({ code: 'TOO_MANY_PEOPLE', path: '/people' });
  if (profile.terms.length > MAX_TERMS) issues.push({ code: 'TOO_MANY_TERMS', path: '/terms' });
  validateOptional(profile.fixed_meeting_mechanism, '/fixed_meeting_mechanism', MAX_MECHANISM_CHARS, issues);

  profile.people.forEach((person, index) => {
    const path = `/people/${index}`;
    validateRequired(person.person_id, `${path}/person_id`, MAX_NAME_CHARS, issues);
    validateRequired(person.display_name, `${path}/display_name`, MAX_NAME_CHARS, issues);
    validateAliases(person.aliases, `${path}/aliases`, issues);
    validateOptional(person.department, `${path}/department`, MAX_OPTIONAL_CHARS, issues);
    validateOptional(person.role, `${path}/role`, MAX_OPTIONAL_CHARS, issues);
  });
  profile.terms.forEach((term, index) => {
    const path = `/terms/${index}`;
    validateRequired(term.term_id, `${path}/term_id`, MAX_NAME_CHARS, issues);
    validateRequired(term.canonical, `${path}/canonical`, MAX_NAME_CHARS, issues);
    validateAliases(term.aliases, `${path}/aliases`, issues);
    validateOptional(term.category, `${path}/category`, MAX_OPTIONAL_CHARS, issues);
  });

  validateUniqueProfiles(
    profile.people.map((person) => ({
      id: person.person_id,
      name: person.display_name,
      aliases: person.aliases,
      enabled: person.enabled,
    })),
    '/people',
    'person_id',
    'display_name',
    {
      duplicateId: 'DUPLICATE_PERSON_ID',
      duplicateName: 'DUPLICATE_PERSON_NAME',
      ambiguousAlias: 'AMBIGUOUS_PERSON_ALIAS',
      aliasMatchesName: 'ALIAS_MATCHES_PERSON_NAME',
    },
    issues,
  );
  validateUniqueProfiles(
    profile.terms.map((term) => ({
      id: term.term_id,
      name: term.canonical,
      aliases: term.aliases,
      enabled: term.enabled,
    })),
    '/terms',
    'term_id',
    'canonical',
    {
      duplicateId: 'DUPLICATE_TERM_ID',
      duplicateName: 'DUPLICATE_TERM_NAME',
      ambiguousAlias: 'AMBIGUOUS_TERM_ALIAS',
      aliasMatchesName: 'ALIAS_MATCHES_TERM_NAME',
    },
    issues,
  );

  return issues;
}

export function createMeetingContextId(prefix: 'person' | 'term'): string {
  const random = typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID().replaceAll('-', '')
    : `${Date.now()}${Math.random().toString(36).slice(2)}`;
  return `${prefix}_${random.slice(0, 24)}`;
}

export function normalizeMeetingContextProfile(
  profile: MeetingContextProfile,
): MeetingContextProfile {
  const optional = (value: string | null): string | null => {
    const trimmed = value?.trim() ?? '';
    return trimmed || null;
  };
  return {
    schema_version: 1,
    fixed_meeting_mechanism: optional(profile.fixed_meeting_mechanism),
    people: profile.people.map((person) => ({
      person_id: person.person_id.trim(),
      display_name: person.display_name.trim(),
      aliases: person.aliases.map((alias) => alias.trim()),
      department: optional(person.department),
      role: optional(person.role),
      enabled: person.enabled,
    })),
    terms: profile.terms.map((term) => ({
      term_id: term.term_id.trim(),
      canonical: term.canonical.trim(),
      aliases: term.aliases.map((alias) => alias.trim()),
      category: optional(term.category),
      enabled: term.enabled,
    })),
  };
}

function canonicalize(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalize);
  if (isRecord(value)) {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
        .map(([key, item]) => [key, canonicalize(item)]),
    );
  }
  return value;
}

export async function meetingContextProfileSha256(
  profile: MeetingContextProfile,
): Promise<string> {
  const bytes = new TextEncoder().encode(JSON.stringify(canonicalize(normalizeMeetingContextProfile(profile))));
  const digest = await crypto.subtle.digest('SHA-256', bytes);
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

export function validateRecordingMeetingContextDraft(
  profile: MeetingContextProfile,
  draft: RecordingMeetingContextDraft,
): MeetingContextProfileIssue[] {
  const enabledPeople = profile.people.filter((person) => person.enabled);
  const enabledTerms = profile.terms.filter((term) => term.enabled);
  const merged: MeetingContextProfile = {
    schema_version: 1,
    fixed_meeting_mechanism: profile.fixed_meeting_mechanism,
    people: [
      ...enabledPeople,
      ...draft.guests.map((guest) => ({
        person_id: guest.personId,
        display_name: guest.displayName,
        aliases: guest.aliases,
        department: guest.department,
        role: guest.role,
        enabled: true,
      })),
    ],
    terms: [
      ...enabledTerms,
      ...draft.additionalTerms.map((term) => ({
        term_id: term.termId,
        canonical: term.canonical,
        aliases: term.aliases,
        category: term.category,
        enabled: true,
      })),
    ],
  };
  const issues = validateMeetingContextProfile(merged);
  const knownIds = new Set(merged.people.map((person) => person.person_id));
  if (draft.hostPersonId && !knownIds.has(draft.hostPersonId)) {
    issues.push({ code: 'HOST_PERSON_NOT_FOUND', path: '/hostPersonId' });
  }
  if (draft.hostPersonId && draft.attendance.some((item) => (
    item.personId === draft.hostPersonId && item.attendance === 'absent'
  ))) {
    issues.push({ code: 'HOST_MARKED_ABSENT', path: '/hostPersonId' });
  }
  const seenAttendance = new Set<string>();
  draft.attendance.forEach((item, index) => {
    if (!knownIds.has(item.personId)) {
      issues.push({ code: 'ATTENDANCE_PERSON_NOT_FOUND', path: `/attendance/${index}/personId` });
    }
    if (seenAttendance.has(item.personId)) {
      issues.push({ code: 'DUPLICATE_ATTENDANCE_OVERRIDE', path: `/attendance/${index}/personId` });
    }
    seenAttendance.add(item.personId);
  });
  return issues;
}
