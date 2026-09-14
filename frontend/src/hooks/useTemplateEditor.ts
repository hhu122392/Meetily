'use client';

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { normalizeTemplateApiError, templateService, type TemplateService } from '@/services/templateService';
import type {
  TemplateApiError,
  TemplateDetails,
  TemplateOrigin,
  TemplateValidationResult,
} from '@/types/summary-template';
import {
  createSaveAsCopyDraft,
  createTemplateDraft,
  editorDraftToTemplate,
  isTemplateDraftDirty,
  templateToEditorDraft,
  validateTemplateDraft,
  type TemplateEditorDraft,
} from '@/lib/template-editor';

export type TemplateEditorMode = 'create' | 'edit';

export interface TemplateEditorLocation {
  mode: TemplateEditorMode;
  templateId: string | null;
  origin: TemplateOrigin | null;
}

function readOnlyError(): TemplateApiError {
  return {
    code: 'TEMPLATE_READ_ONLY',
    messageKey: 'templates.errors.readOnly',
    retryable: false,
    debugId: `template-editor-readonly-${Date.now()}`,
  };
}

function invalidLocationError(): TemplateApiError {
  return {
    code: 'TEMPLATE_INVALID',
    messageKey: 'templates.editor.errors.invalidLocation',
    retryable: false,
    debugId: `template-editor-location-${Date.now()}`,
  };
}

export function useTemplateEditor(
  location: TemplateEditorLocation,
  service: TemplateService = templateService,
) {
  const [draft, setDraft] = useState<TemplateEditorDraft>(() => createTemplateDraft());
  const [baseline, setBaseline] = useState<TemplateEditorDraft>(() => createTemplateDraft());
  const [details, setDetails] = useState<TemplateDetails | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<TemplateApiError | null>(null);
  const [validation, setValidation] = useState<TemplateValidationResult | null>(null);
  const [validating, setValidating] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<TemplateApiError | null>(null);
  const [conflict, setConflict] = useState<TemplateApiError | null>(null);
  const mounted = useRef(true);
  const loadSequence = useRef(0);
  const validationSequence = useRef(0);
  const saveLocked = useRef(false);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const load = useCallback(async () => {
    const sequence = ++loadSequence.current;
    setLoading(true);
    setLoadError(null);
    setSaveError(null);
    setConflict(null);
    setValidation(null);

    if (location.mode === 'create') {
      const initial = createTemplateDraft();
      if (!mounted.current || sequence !== loadSequence.current) return;
      setDraft(initial);
      setBaseline(templateToEditorDraft(editorDraftToTemplate(initial)));
      setDetails(null);
      setLoading(false);
      return;
    }

    if (!location.templateId) {
      if (!mounted.current || sequence !== loadSequence.current) return;
      setLoadError(invalidLocationError());
      setLoading(false);
      return;
    }

    try {
      const loaded = await service.get({
        templateId: location.templateId,
        origin: location.origin ?? undefined,
      });
      if (!mounted.current || sequence !== loadSequence.current) return;
      if (loaded.readOnly || loaded.origin !== 'custom') {
        setLoadError(readOnlyError());
        setLoading(false);
        return;
      }
      const loadedDraft = templateToEditorDraft(loaded.template);
      setDraft(loadedDraft);
      setBaseline(templateToEditorDraft(loaded.template));
      setDetails(loaded);
      setLoading(false);
    } catch (error) {
      if (!mounted.current || sequence !== loadSequence.current) return;
      setLoadError(normalizeTemplateApiError(error));
      setLoading(false);
    }
  }, [location.mode, location.origin, location.templateId, service]);

  useEffect(() => {
    void load();
  }, [load]);

  const localIssues = useMemo(() => validateTemplateDraft(draft), [draft]);
  const dirty = useMemo(() => isTemplateDraftDirty(draft, baseline), [baseline, draft]);

  useEffect(() => {
    if (loading) return;
    const sequence = ++validationSequence.current;

    if (localIssues.length > 0) {
      setValidation(null);
      setValidating(false);
      return;
    }

    setValidating(true);
    const timeout = globalThis.setTimeout(async () => {
      try {
        const result = await service.validate(
          editorDraftToTemplate(draft),
          location.mode === 'create' ? 'create' : 'update',
        );
        if (!mounted.current || sequence !== validationSequence.current) return;
        setValidation(result);
      } catch (error) {
        if (!mounted.current || sequence !== validationSequence.current) return;
        const normalized = normalizeTemplateApiError(error);
        setValidation({
          valid: false,
          errors: normalized.fieldErrors?.length
            ? normalized.fieldErrors
            : [{
                code: normalized.code,
                path: '',
                messageKey: normalized.messageKey,
                params: normalized.params,
              }],
          warnings: [],
          normalized: null,
        });
      } finally {
        if (mounted.current && sequence === validationSequence.current) setValidating(false);
      }
    }, 350);

    return () => globalThis.clearTimeout(timeout);
  }, [draft, loading, localIssues.length, location.mode, service]);

  const validateCurrentDraft = useCallback(async (): Promise<TemplateValidationResult> => {
    const local = validateTemplateDraft(draft);
    if (local.length > 0) {
      return { valid: false, errors: local, warnings: [], normalized: null };
    }
    const result = await service.validate(
      editorDraftToTemplate(draft),
      location.mode === 'create' ? 'create' : 'update',
    );
    if (mounted.current) setValidation(result);
    return result;
  }, [draft, location.mode, service]);

  const save = useCallback(async (): Promise<TemplateDetails | null> => {
    if (saveLocked.current) return null;
    saveLocked.current = true;
    setSaving(true);
    setSaveError(null);
    setConflict(null);

    try {
      const currentValidation = await validateCurrentDraft();
      if (!currentValidation.valid) return null;

      const template = editorDraftToTemplate(draft);
      let result: TemplateDetails;
      if (location.mode === 'create') {
        result = await service.create({ template, conflictPolicy: 'error' });
      } else {
        if (!details) throw invalidLocationError();
        result = await service.update({
          templateId: details.template.id,
          expectedVersion: details.template.version,
          expectedFileSha256: details.fileSha256,
          template,
        });
      }

      if (mounted.current) {
        const savedDraft = templateToEditorDraft(result.template);
        setDraft(savedDraft);
        setBaseline(templateToEditorDraft(result.template));
        setDetails(result);
        setValidation({ valid: true, errors: [], warnings: [], normalized: result.template });
      }
      return result;
    } catch (error) {
      const normalized = normalizeTemplateApiError(error);
      if (mounted.current) {
        if (normalized.code === 'TEMPLATE_CONFLICT' || normalized.code === 'TEMPLATE_ALREADY_EXISTS') {
          setConflict(normalized);
        } else {
          setSaveError(normalized);
        }
      }
      return null;
    } finally {
      saveLocked.current = false;
      if (mounted.current) setSaving(false);
    }
  }, [details, draft, location.mode, service, validateCurrentDraft]);

  const saveAsCopy = useCallback(async (
    id: string,
    name: string,
  ): Promise<TemplateDetails | null> => {
    if (saveLocked.current) return null;
    saveLocked.current = true;
    setSaving(true);
    setSaveError(null);

    try {
      const copyDraft = createSaveAsCopyDraft(draft, id, name);
      const local = validateTemplateDraft(copyDraft);
      if (local.length > 0) {
        setValidation({ valid: false, errors: local, warnings: [], normalized: null });
        return null;
      }
      const validated = await service.validate(editorDraftToTemplate(copyDraft), 'create');
      setValidation(validated);
      if (!validated.valid) return null;
      const result = await service.create({
        template: editorDraftToTemplate(copyDraft),
        conflictPolicy: 'error',
      });
      if (mounted.current) {
        const savedDraft = templateToEditorDraft(result.template);
        setDraft(savedDraft);
        setBaseline(templateToEditorDraft(result.template));
        setDetails(result);
        setConflict(null);
      }
      return result;
    } catch (error) {
      const normalized = normalizeTemplateApiError(error);
      if (mounted.current) {
        if (normalized.code === 'TEMPLATE_CONFLICT' || normalized.code === 'TEMPLATE_ALREADY_EXISTS') {
          setConflict(normalized);
        } else {
          setSaveError(normalized);
        }
      }
      return null;
    } finally {
      saveLocked.current = false;
      if (mounted.current) setSaving(false);
    }
  }, [draft, service]);

  const applyJsonValue = useCallback(async (value: unknown): Promise<boolean> => {
    if (saveLocked.current) return false;
    saveLocked.current = true;
    setValidating(true);
    try {
      const result = await service.validate(
        value,
        location.mode === 'create' ? 'create' : 'update',
      );
      if (!mounted.current) return false;
      setValidation(result);
      if (!result.valid || !result.normalized) return false;
      if (location.mode === 'edit' && details && result.normalized.id !== details.template.id) {
        setValidation({
          valid: false,
          errors: [{
            code: 'EDITOR_ID_IMMUTABLE',
            path: '/id',
            messageKey: 'templates.editor.validation.idImmutable',
          }],
          warnings: result.warnings,
          normalized: null,
        });
        return false;
      }
      setDraft(templateToEditorDraft(result.normalized));
      return true;
    } catch (error) {
      if (mounted.current) setSaveError(normalizeTemplateApiError(error));
      return false;
    } finally {
      saveLocked.current = false;
      if (mounted.current) setValidating(false);
    }
  }, [details, location.mode, service]);

  const clearConflict = useCallback(() => setConflict(null), []);
  const clearValidation = useCallback(() => setValidation(null), []);

  return {
    draft,
    setDraft,
    baseline,
    details,
    loading,
    loadError,
    localIssues,
    validation,
    validating,
    saving,
    saveError,
    conflict,
    dirty,
    load,
    save,
    saveAsCopy,
    applyJsonValue,
    clearConflict,
    clearValidation,
  };
}
