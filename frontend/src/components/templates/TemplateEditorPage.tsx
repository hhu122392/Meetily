'use client';

import { HelpHint } from '@/components/ui/help-hint';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useRouter, useSearchParams } from 'next/navigation';
import { useTranslation } from 'react-i18next';
import { toast } from 'sonner';
import {
  AlertCircle,
  ArrowLeft,
  Braces,
  CheckCircle2,
  ChevronDown,
  Clipboard,
  Code2,
  FileText,
  RefreshCw,
  Save,
  TriangleAlert,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { TemplateEditorFields } from '@/components/templates/TemplateEditorFields';
import { TemplateStructurePreview } from '@/components/templates/TemplateStructurePreview';
import { useTemplateEditor } from '@/hooks/useTemplateEditor';
import {
  formatEditorJson,
  parseEditorJson,
  slugifyTemplateId,
} from '@/lib/template-editor';
import { templateTranslationKey } from '@/lib/template-library';
import {
  APP_NAVIGATION_REQUEST_EVENT,
  requestAppNavigation,
  type AppNavigationRequestDetail,
} from '@/lib/navigation-guard';
import type { TemplateFieldIssue, TemplateOrigin } from '@/types/summary-template';

type EditorTab = 'form' | 'json';

interface PendingNavigation {
  destination: string;
  proceed: () => void;
}

function parseOrigin(value: string | null): TemplateOrigin | null {
  return value === 'custom' || value === 'builtin' || value === 'bundled' ? value : null;
}

export function TemplateEditorPage() {
  const router = useRouter();
  const searchParams = useSearchParams();
  const { t } = useTranslation('templates');
  const mode = searchParams.get('mode') === 'edit' ? 'edit' : 'create';
  const templateId = searchParams.get('id');
  const origin = parseOrigin(searchParams.get('origin'));
  const editor = useTemplateEditor({ mode, templateId, origin });
  const [tab, setTab] = useState<EditorTab>('form');
  const [jsonText, setJsonText] = useState('');
  const [jsonBaseline, setJsonBaseline] = useState('');
  const [jsonSyntaxError, setJsonSyntaxError] = useState<string | null>(null);
  const [jsonSwitchOpen, setJsonSwitchOpen] = useState(false);
  const [pendingNavigation, setPendingNavigation] = useState<PendingNavigation | null>(null);
  const [copyDialogOpen, setCopyDialogOpen] = useState(false);
  const [copyId, setCopyId] = useState('');
  const [copyName, setCopyName] = useState('');
  // 新表单刚打开时不要摆出一堆红字：只有点过「保存模板」之后才提示校验错误
  const [validationRevealed, setValidationRevealed] = useState(false);
  const autoId = useRef(true);
  const idSuffix = useRef(Date.now().toString(36).slice(-8));

  const jsonDirty = tab === 'json' && jsonText !== jsonBaseline;
  const overallDirty = editor.dirty || jsonDirty;
  const copyIdValid = /^[a-z0-9][a-z0-9_-]*[a-z0-9]$/.test(copyId.trim())
    && copyId.trim().length >= 3
    && copyId.trim().length <= 80;
  const copyNameValid = copyName.trim().length >= 1 && copyName.length <= 120;
  const allIssues = useMemo(() => {
    const issues: TemplateFieldIssue[] = [...editor.localIssues];
    for (const issue of editor.validation?.errors ?? []) {
      if (!issues.some((candidate) => candidate.path === issue.path && candidate.code === issue.code)) {
        issues.push(issue);
      }
    }
    return issues;
  }, [editor.localIssues, editor.validation?.errors]);

  useEffect(() => {
    autoId.current = mode === 'create';
    idSuffix.current = Date.now().toString(36).slice(-8);
  }, [mode, templateId]);

  useEffect(() => {
    const handleBeforeUnload = (event: BeforeUnloadEvent) => {
      if (!overallDirty) return;
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('beforeunload', handleBeforeUnload);
    return () => window.removeEventListener('beforeunload', handleBeforeUnload);
  }, [overallDirty]);

  useEffect(() => {
    const handleAppNavigation = (rawEvent: Event) => {
      if (!overallDirty) return;
      const event = rawEvent as CustomEvent<AppNavigationRequestDetail>;
      if (!event.detail?.destination || typeof event.detail.proceed !== 'function') return;
      event.preventDefault();
      setPendingNavigation({
        destination: event.detail.destination,
        proceed: event.detail.proceed,
      });
    };
    window.addEventListener(APP_NAVIGATION_REQUEST_EVENT, handleAppNavigation);
    return () => window.removeEventListener(APP_NAVIGATION_REQUEST_EVENT, handleAppNavigation);
  }, [overallDirty]);

  useEffect(() => {
    const captureNavigation = (event: MouseEvent) => {
      if (!overallDirty || event.defaultPrevented || event.button !== 0) return;
      const target = event.target;
      if (!(target instanceof Element)) return;
      const anchor = target.closest<HTMLAnchorElement>('a[href]');
      if (!anchor || anchor.target === '_blank' || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const destination = new URL(anchor.href, window.location.href);
      if (destination.origin !== window.location.origin || destination.href === window.location.href) return;
      event.preventDefault();
      event.stopPropagation();
      const path = `${destination.pathname}${destination.search}${destination.hash}`;
      setPendingNavigation({ destination: path, proceed: () => router.push(path) });
    };
    document.addEventListener('click', captureNavigation, true);
    return () => document.removeEventListener('click', captureNavigation, true);
  }, [overallDirty, router]);

  const requestNavigation = useCallback((destination: string) => {
    requestAppNavigation(destination, () => router.push(destination));
  }, [router]);

  const handleNameChange = (name: string) => {
    editor.setDraft((current) => ({
      ...current,
      name,
      id: mode === 'create' && autoId.current
        ? slugifyTemplateId(name, idSuffix.current)
        : current.id,
    }));
  };

  const handleIdChange = (id: string) => {
    autoId.current = false;
    editor.setDraft((current) => ({ ...current, id }));
  };

  const enterJsonMode = () => {
    const formatted = formatEditorJson(editor.draft);
    setJsonText(formatted);
    setJsonBaseline(formatted);
    setJsonSyntaxError(null);
    setTab('json');
  };

  const requestFormMode = () => {
    if (jsonDirty) setJsonSwitchOpen(true);
    else setTab('form');
  };

  const applyJson = async (): Promise<boolean> => {
    const parsed = parseEditorJson(jsonText);
    if (parsed.error) {
      setJsonSyntaxError(parsed.error);
      return false;
    }
    setJsonSyntaxError(null);
    const applied = await editor.applyJsonValue(parsed.value);
    if (!applied) {
      return false;
    }
    const formatted = `${JSON.stringify(parsed.value, null, 2)}\n`;
    setJsonText(formatted);
    setJsonBaseline(formatted);
    toast.success(t('editor.json.applySuccess'));
    return true;
  };

  const handleFormatJson = () => {
    const parsed = parseEditorJson(jsonText);
    if (parsed.error) {
      setJsonSyntaxError(parsed.error);
      return;
    }
    setJsonSyntaxError(null);
    setJsonText(`${JSON.stringify(parsed.value, null, 2)}\n`);
  };

  const handleCopyJson = async () => {
    try {
      await navigator.clipboard.writeText(jsonText);
      toast.success(t('editor.json.copied'));
    } catch {
      toast.error(t('errors.io'));
    }
  };

  const handleSave = async () => {
    if (jsonDirty) {
      toast.error(t('editor.leave.applyJsonFirst'));
      return;
    }
    setValidationRevealed(true);
    const result = await editor.save();
    if (!result) return;
    toast.success(t('editor.saveSuccess'));
    const target = `/settings/templates/editor?mode=edit&id=${encodeURIComponent(result.template.id)}&origin=custom`;
    if (mode === 'create' || templateId !== result.template.id) router.replace(target);
  };

  const navigateWithoutGuard = (navigation: PendingNavigation) => {
    setPendingNavigation(null);
    navigation.proceed();
  };

  const handleSaveAndLeave = async () => {
    if (!pendingNavigation || jsonDirty) return;
    const navigation = pendingNavigation;
    setValidationRevealed(true);
    const result = await editor.save();
    if (result) navigateWithoutGuard(navigation);
  };

  const openSaveCopy = () => {
    const suggestedId = slugifyTemplateId(`${editor.draft.id}_copy`, idSuffix.current);
    setCopyId(suggestedId === editor.draft.id ? `${editor.draft.id}_copy`.slice(0, 80) : suggestedId);
    setCopyName(`${editor.draft.name || editor.draft.id} Copy`);
    editor.clearConflict();
    setCopyDialogOpen(true);
  };

  const handleSaveCopy = async () => {
    setValidationRevealed(true);
    const result = await editor.saveAsCopy(copyId.trim(), copyName.trim());
    if (!result) return;
    setCopyDialogOpen(false);
    editor.clearConflict();
    toast.success(t('editor.saveSuccess'));
    router.replace(`/settings/templates/editor?mode=edit&id=${encodeURIComponent(result.template.id)}&origin=custom`);
  };

  if (editor.loading) {
    return <EditorLoading label={t('editor.loading')} />;
  }

  if (editor.loadError) {
    return (
      <EditorLoadError
        title={t('editor.loadFailed')}
        detail={t(templateTranslationKey(editor.loadError.messageKey))}
        retryable={editor.loadError.retryable}
        backLabel={t('editor.back')}
        retryLabel={t('editor.retry')}
        onBack={() => router.push('/settings/templates')}
        onRetry={() => void editor.load()}
      />
    );
  }

  return (
    <div className="flex h-screen min-w-0 flex-col bg-gray-50 text-gray-900">
      <header className="z-10 shrink-0 border-b border-gray-200 bg-white">
        <div className="mx-auto flex w-full max-w-[1500px] flex-wrap items-center justify-between gap-3 px-4 py-3 sm:px-6">
          <div className="flex min-w-0 items-center gap-3">
            <Button type="button" variant="ghost" size="sm" onClick={() => requestNavigation('/settings/templates')}>
              <ArrowLeft aria-hidden="true" />
              {t('editor.back')}
            </Button>
            <div className="min-w-0 border-l border-gray-200 pl-3">
              <h1 className="truncate text-xl font-bold">{mode === 'create' ? t('editor.titleCreate') : t('editor.titleEdit')}</h1>
              <div className="mt-0.5 flex flex-wrap items-center gap-2 text-xs text-gray-500">
                <span className={overallDirty ? 'font-medium text-amber-700' : 'text-emerald-700'}>
                  {overallDirty ? t('editor.unsaved') : t('editor.saved')}
                </span>
                {editor.details && <span>· {t('editor.version', { version: editor.details.template.version })}</span>}
              </div>
            </div>
          </div>
          <Button
            type="button"
            onClick={() => void handleSave()}
            // 不要在"有校验错误"时把按钮灰掉：那样用户点都点不了，也就永远看不到为什么不能保存。
            // 新建模式即使还没改过也允许点，点下去会给出校验提示。
            disabled={editor.saving || editor.validating || (!editor.dirty && mode !== 'create')}
          >
            {editor.saving ? <RefreshCw className="animate-spin" aria-hidden="true" /> : <Save aria-hidden="true" />}
            {editor.saving ? t('editor.saving') : t('editor.save')}
          </Button>
        </div>
      </header>

      <main className="min-h-0 flex-1 overflow-y-auto custom-scrollbar">
        <div className="mx-auto w-full max-w-[1500px] px-4 py-5 sm:px-6">

          <div className="mb-4 flex items-center gap-1 border-b border-gray-200" role="tablist" aria-label={t('editor.titleEdit')}>
            <EditorTabButton active={tab === 'form'} onClick={requestFormMode} icon={<FileText />} label={t('editor.tabs.form')} />
            <EditorTabButton active={tab === 'json'} onClick={enterJsonMode} icon={<Code2 />} label={t('editor.tabs.json')} />
            <HelpHint text={t('editor.description')} />
          </div>

          <ValidationSummary
            issues={allIssues}
            validation={editor.validation}
            validating={editor.validating}
            saveError={editor.saveError}
            revealed={validationRevealed}
          />

          {tab === 'form' ? (
            <div className="grid items-start gap-5 xl:grid-cols-[minmax(0,3fr)_minmax(320px,2fr)]">
              <TemplateEditorFields
                draft={editor.draft}
                setDraft={editor.setDraft}
                issues={validationRevealed ? allIssues : []}
                isCreate={mode === 'create'}
                onNameChange={handleNameChange}
                onIdChange={handleIdChange}
              />
              <div className="xl:sticky xl:top-5">
                <TemplateStructurePreview draft={editor.draft} />
              </div>
            </div>
          ) : (
            <section className="rounded-lg border border-gray-200 bg-white shadow-sm">
              <div className="flex flex-wrap items-center justify-between gap-3 border-b border-gray-100 p-4">
                <div>
                  <h2 className="flex items-center gap-2 font-semibold"><Braces className="h-4 w-4 text-blue-600" aria-hidden="true" />{t('editor.tabs.json')}<HelpHint text={t('editor.json.description')} /></h2>
                </div>
                <div className="flex flex-wrap gap-2">
                  <Button type="button" variant="outline" size="sm" onClick={handleFormatJson}>{t('editor.json.format')}</Button>
                  <Button type="button" variant="outline" size="sm" onClick={() => void handleCopyJson()}><Clipboard aria-hidden="true" />{t('editor.json.copy')}</Button>
                  <Button type="button" size="sm" onClick={() => void applyJson()} disabled={!jsonDirty || editor.validating}>
                    {editor.validating && <RefreshCw className="animate-spin" aria-hidden="true" />}
                    {editor.validating ? t('editor.json.applying') : t('editor.json.apply')}
                  </Button>
                </div>
              </div>
              <div className="p-4">
                {jsonDirty && (
                  <p className="mb-3 py-2 text-sm text-amber-800" role="status">
                    {t('editor.json.notAppliedDescription')}
                  </p>
                )}
                <Textarea
                  value={jsonText}
                  onChange={(event) => {
                    setJsonText(event.target.value);
                    setJsonSyntaxError(null);
                    editor.clearValidation();
                  }}
                  spellCheck={false}
                  aria-invalid={!!jsonSyntaxError}
                  className="min-h-[520px] resize-y whitespace-pre font-mono text-xs leading-5"
                />
                {jsonSyntaxError && (
                  <p className="mt-3 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-700" role="alert">
                    {t('editor.json.invalid', { error: jsonSyntaxError })}
                  </p>
                )}
              </div>
            </section>
          )}
        </div>
      </main>

      <Dialog open={jsonSwitchOpen} onOpenChange={setJsonSwitchOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('editor.json.notApplied')}</DialogTitle>
            <DialogDescription>{t('editor.json.notAppliedDescription')}</DialogDescription>
          </DialogHeader>
          <DialogFooter className="sm:flex-wrap">
            <Button type="button" variant="outline" onClick={() => setJsonSwitchOpen(false)}>{t('editor.json.continue')}</Button>
            <Button type="button" variant="outline" onClick={() => {
                        setJsonText(jsonBaseline);
                        setJsonSyntaxError(null);
                        editor.clearValidation();
                        setJsonSwitchOpen(false);
                        setTab('form');
            }}>{t('editor.json.discard')}</Button>
            <Button type="button" onClick={async () => {
              if (await applyJson()) {
                setJsonSwitchOpen(false);
                setTab('form');
              }
            }}>{t('editor.json.apply')}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={pendingNavigation !== null} onOpenChange={(open) => !open && setPendingNavigation(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('editor.leave.title')}</DialogTitle>
            <DialogDescription>{t('editor.leave.description')}</DialogDescription>
          </DialogHeader>
          {jsonDirty && (
            <div className="py-2 text-sm text-amber-800" role="alert">
              {t('editor.leave.applyJsonFirst')}
            </div>
          )}
          <DialogFooter className="sm:flex-wrap">
            <Button type="button" variant="outline" onClick={() => setPendingNavigation(null)}>{t('editor.leave.continue')}</Button>
            <Button type="button" variant="destructive" onClick={() => pendingNavigation && navigateWithoutGuard(pendingNavigation)}>{t('editor.leave.discard')}</Button>
            <Button type="button" onClick={() => void handleSaveAndLeave()} disabled={jsonDirty || editor.saving || allIssues.length > 0}>
              {editor.saving && <RefreshCw className="animate-spin" aria-hidden="true" />}
              {t('editor.leave.save')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={editor.conflict !== null && !copyDialogOpen} onOpenChange={(open) => !open && editor.clearConflict()}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('editor.conflict.title')}</DialogTitle>
            <DialogDescription>{t('editor.conflict.description')}</DialogDescription>
          </DialogHeader>
          <div className="py-2 text-sm text-amber-800" role="alert">
            <p>{t(templateTranslationKey(editor.conflict?.messageKey ?? 'templates.errors.conflict'))}</p>
            <p className="mt-1 select-all font-mono text-xs">{editor.conflict?.debugId}</p>
          </div>
          <DialogFooter className="sm:flex-wrap">
            <Button type="button" variant="outline" onClick={editor.clearConflict}>{t('editor.conflict.continue')}</Button>
            <Button type="button" variant="outline" onClick={openSaveCopy}>{t('editor.conflict.saveCopy')}</Button>
            {mode === 'edit' && (
              <Button type="button" variant="destructive" title={t('editor.conflict.reloadWarning')} onClick={() => void editor.load()}>
                <RefreshCw aria-hidden="true" />
                {t('editor.conflict.reload')}
              </Button>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={copyDialogOpen} onOpenChange={(open) => {
        setCopyDialogOpen(open);
        if (!open) editor.clearConflict();
      }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('editor.conflict.copyTitle')}</DialogTitle>
            <DialogDescription>{t('editor.conflict.copyDescription')}</DialogDescription>
          </DialogHeader>
          <label className="space-y-2 text-sm font-medium">
            <span>{t('editor.conflict.copyId')}</span>
            <Input value={copyId} maxLength={80} aria-invalid={!copyIdValid} onChange={(event) => setCopyId(event.target.value.toLocaleLowerCase())} className="font-mono" />
            {!copyIdValid && <p className="text-xs text-red-600" role="alert">{t('editor.validation.id')}</p>}
          </label>
          <label className="space-y-2 text-sm font-medium">
            <span>{t('editor.conflict.copyName')}</span>
            <Input value={copyName} maxLength={120} aria-invalid={!copyNameValid} onChange={(event) => setCopyName(event.target.value)} />
            {!copyNameValid && <p className="text-xs text-red-600" role="alert">{t('editor.validation.name')}</p>}
          </label>
          {editor.saveError && <InlineError error={editor.saveError} />}
          {editor.conflict && <InlineError error={editor.conflict} />}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setCopyDialogOpen(false)}>{t('duplicate.cancel')}</Button>
            <Button type="button" onClick={() => void handleSaveCopy()} disabled={editor.saving || !copyIdValid || !copyNameValid}>
              {editor.saving && <RefreshCw className="animate-spin" aria-hidden="true" />}
              {t('editor.conflict.confirmCopy')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function ValidationSummary({
  issues,
  validation,
  validating,
  saveError,
  revealed,
}: {
  issues: readonly TemplateFieldIssue[];
  validation: ReturnType<typeof useTemplateEditor>['validation'];
  validating: boolean;
  saveError: ReturnType<typeof useTemplateEditor>['saveError'];
  /** 只有点过「保存模板」之后才提示校验错误，避免新表单刚打开就被判错 */
  revealed: boolean;
}) {
  const { t } = useTranslation('templates');
  const [detailsOpen, setDetailsOpen] = useState(false);
  if (saveError) return <InlineError error={saveError} />;
  if (revealed && issues.length > 0) {
    const summaries = issues
      .slice(0, 3)
      .map((issue) => t(templateTranslationKey(issue.messageKey), issue.params ?? {}));
    return (
      <div className="mb-3 overflow-hidden rounded-md border border-red-200 bg-red-50 text-red-800">
        <button
          type="button"
          onClick={() => setDetailsOpen((open) => !open)}
          aria-expanded={detailsOpen}
          aria-controls="template-validation-details"
          className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm transition-colors hover:bg-red-100/60"
        >
          <AlertCircle className="h-4 w-4 flex-none" aria-hidden="true" />
          <span className="whitespace-nowrap font-medium">
            {t('editor.validation.summary', { count: issues.length })}
          </span>
          <span className="hidden min-w-0 flex-1 truncate text-red-700 md:block">
            · {summaries.join('、')}
          </span>
          <ChevronDown
            className={`ml-auto h-4 w-4 flex-none transition-transform ${detailsOpen ? 'rotate-180' : ''}`}
            aria-hidden="true"
          />
        </button>
        {detailsOpen && (
          <ul
            id="template-validation-details"
            className="space-y-1 border-t border-red-200 px-3 py-2 pl-8 text-sm"
          >
            {issues.slice(0, 8).map((issue, index) => (
              <li key={`${issue.path}:${issue.code}:${index}`}>
                {t(templateTranslationKey(issue.messageKey), issue.params ?? {})}
              </li>
            ))}
          </ul>
        )}
      </div>
    );
  }
  if (validating) {
    return <div className="mb-4 flex items-center gap-2 rounded-md border border-blue-200 bg-blue-50 p-3 text-sm text-blue-800" role="status"><RefreshCw className="h-4 w-4 animate-spin" aria-hidden="true" />{t('editor.validation.validating')}</div>;
  }
  if (validation?.warnings.length) {
    return (
      <div className="mb-4 py-2 text-sm text-amber-800" role="status">
        {validation.warnings.map((warning, index) => <div key={`${warning.path}:${warning.code}:${index}`} className="flex items-center gap-2"><TriangleAlert className="h-4 w-4" aria-hidden="true" />{t(templateTranslationKey(warning.messageKey), warning.params ?? {})}</div>)}
      </div>
    );
  }
  if (validation?.valid) {
    return <div className="mb-4 flex items-center gap-2 text-xs text-emerald-700" role="status"><CheckCircle2 className="h-4 w-4" aria-hidden="true" />{t('editor.validation.valid')}</div>;
  }
  return null;
}

function InlineError({ error }: { error: NonNullable<ReturnType<typeof useTemplateEditor>['saveError']> }) {
  const { t } = useTranslation('templates');
  return (
    <div className="mb-4 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-800" role="alert">
      <p>{t(templateTranslationKey(error.messageKey))}</p>
      <p className="mt-1 select-all font-mono text-xs text-red-600">{error.debugId}</p>
    </div>
  );
}

function EditorTabButton({ active, onClick, icon, label }: { active: boolean; onClick: () => void; icon: React.ReactNode; label: string }) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      onClick={onClick}
      className={`flex items-center gap-2 border-b-2 px-4 py-2.5 text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 [&>svg]:h-4 [&>svg]:w-4 ${active ? 'border-blue-600 text-blue-700' : 'border-transparent text-gray-600 hover:text-gray-900'}`}
    >
      {icon}{label}
    </button>
  );
}

function EditorLoading({ label }: { label: string }) {
  return <div className="flex h-screen items-center justify-center gap-3 bg-gray-50 text-gray-600"><RefreshCw className="h-5 w-5 animate-spin" aria-hidden="true" />{label}</div>;
}

function EditorLoadError({ title, detail, retryable, backLabel, retryLabel, onBack, onRetry }: { title: string; detail: string; retryable: boolean; backLabel: string; retryLabel: string; onBack: () => void; onRetry: () => void }) {
  return (
    <div className="flex h-screen items-center justify-center bg-gray-50 p-6">
      <div className="w-full max-w-lg rounded-lg border border-red-200 bg-white p-6 text-center shadow-sm" role="alert">
        <AlertCircle className="mx-auto h-9 w-9 text-red-600" aria-hidden="true" />
        <h1 className="mt-3 text-lg font-semibold">{title}</h1>
        <p className="mt-2 text-sm text-gray-600">{detail}</p>
        <div className="mt-5 flex justify-center gap-2">
          <Button type="button" variant="outline" onClick={onBack}><ArrowLeft aria-hidden="true" />{backLabel}</Button>
          {retryable && <Button type="button" onClick={onRetry}><RefreshCw aria-hidden="true" />{retryLabel}</Button>}
        </div>
      </div>
    </div>
  );
}
