'use client';

import { HelpHint } from '@/components/ui/help-hint';
import { useMemo, useState } from 'react';
import { Check, Loader2, Sparkles, Wand2, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { hasConflictingTranscriptEdits, transcriptCandidateKey } from '@/lib/transcript-edit-selection';
import { Button } from '../ui/button';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '../ui/select';
import type {
  CorrectionCandidate,
  ProofreadCandidate,
  ProofreadModelOption,
  ProofreadResponse,
  ProofreadTarget,
} from '@/lib/transcript-revision';

const SOURCE_KIND_LABEL_KEYS = {
  rules: 'correction.sourceKindRules',
  local: 'correction.sourceKindLocal',
  api: 'correction.sourceKindApi',
} as const;

const SOURCE_KIND_STYLES: Record<'rules' | 'local' | 'api', string> = {
  rules: 'bg-gray-100 text-gray-600 border-gray-200',
  local: 'bg-amber-50 text-amber-800 border-amber-200',
  api: 'bg-blue-50 text-blue-700 border-blue-200',
};

/** 下拉需要一个稳定的字符串 key */
function targetKey(target: ProofreadTarget): string {
  return target.kind === 'summary' ? 'summary' : `provider:${target.provider}:${target.model}`;
}

interface TranscriptCorrectionPanelProps {
  /** 规则层 + AI 层合并后的候选（同一场会议、同一份原文本） */
  candidates: CorrectionCandidate[];
  /** 其中来自规则/术语层的条数（其余来自 AI） */
  ruleCandidateCount: number;
  /** 规则词典命中的规则 id（放 tooltip，避免正文里堆一长串） */
  appliedRules: string[];
  /** 规则层用到的会议上下文规模（没候选时用它解释"为什么没命中"） */
  contextPersonCount: number;
  contextTermCount: number;
  /** 每一轮 AI 深查的结果（本地一轮 + 可选 API 复查一轮） */
  aiRuns: ProofreadResponse[];
  isRunningAi: boolean;
  isApplying: boolean;
  isSavingModel?: boolean;
  /** 这台机器上可选的校对模型（摘要模型 + 已配置的 API 供应商） */
  modelOptions: ProofreadModelOption[];
  /** 当前选中的校对模型 */
  selectedTarget: ProofreadTarget;
  onRunAi: () => void;
  /** 换一个校对模型（会记住） */
  onTargetChange: (target: ProofreadTarget) => void;
  onApply: (selected: ProofreadCandidate[]) => void;
  onCancel: () => void;
}

/** 把文本按某段片段切片渲染（只在候选位置做高亮，其余保持原样） */
function SnippetHighlight({
  text,
  snippet,
  startChar,
  tone,
}: {
  text: string;
  snippet: string;
  startChar: number;
  tone: 'before' | 'after';
}) {
  const chars = Array.from(text);
  const length = Array.from(snippet).length;
  if (chars.slice(startChar, startChar + length).join('') !== snippet) {
    return <span>{text}</span>;
  }
  return (
    <span>
      {chars.slice(0, startChar).join('')}
      <mark
        className={
          tone === 'before'
            ? 'rounded bg-red-100 px-0.5 text-red-900'
            : 'rounded bg-green-100 px-0.5 text-green-900'
        }
      >
        {snippet}
      </mark>
      {chars.slice(startChar + length).join('')}
    </span>
  );
}

function formatSeconds(value: number | null): string {
  if (value === null || Number.isNaN(value)) return '--:--';
  const total = Math.max(0, Math.floor(value));
  return `${String(Math.floor(total / 60)).padStart(2, '0')}:${String(total % 60).padStart(2, '0')}`;
}

const CONFIDENCE_STYLES: Record<string, string> = {
  high: 'bg-green-100 text-green-800 border-green-200',
  medium: 'bg-amber-100 text-amber-900 border-amber-200',
  low: 'bg-gray-100 text-gray-600 border-gray-200',
};

/** 来源标签：规则层给 rules/context，AI 给 homophone/term/typo */
const SOURCE_LABEL_KEYS = {
  rules: 'correction.sourceRules',
  context: 'correction.sourceContext',
  homophone: 'correction.sourceHomophone',
  term: 'correction.sourceTerm',
  typo: 'correction.sourceTypo',
} as const;

type SourceReason = keyof typeof SOURCE_LABEL_KEYS;

const SOURCE_STYLES: Record<string, string> = {
  rules: 'bg-blue-50 text-blue-700 border-blue-200',
  context: 'bg-indigo-50 text-indigo-700 border-indigo-200',
  homophone: 'bg-green-50 text-green-700 border-green-200',
  term: 'bg-green-50 text-green-700 border-green-200',
  typo: 'bg-green-50 text-green-700 border-green-200',
};

/**
 * 「文字纠错」候选清单：规则层（秒级）和 AI 深查的候选放同一个列表，
 * 勾选后一次写回、一次备份。默认全选 —— 跑这两步都不写库，只有点"应用"才动文本。
 */
export function TranscriptCorrectionPanel({
  candidates,
  ruleCandidateCount,
  appliedRules,
  contextPersonCount,
  contextTermCount,
  aiRuns,
  isRunningAi,
  isApplying,
  isSavingModel = false,
  modelOptions,
  selectedTarget,
  onRunAi,
  onTargetChange,
  onApply,
  onCancel,
}: TranscriptCorrectionPanelProps) {
  const { t } = useTranslation('transcription');
  // 记"被取消勾选的"，这样 AI 深查追加进来的新候选默认也是选中的
  const [deselected, setDeselected] = useState<Set<string>>(() => new Set());

  // 同一条改动可能出现两次（同一片段里同一个词改两次），用序号区分，保证 key 稳定
  const keyed = useMemo(() => {
    const seen = new Map<string, number>();
    return candidates.map((candidate) => {
      const base = transcriptCandidateKey(candidate);
      const ordinal = seen.get(base) ?? 0;
      seen.set(base, ordinal + 1);
      return { key: `${base}#${ordinal}`, candidate };
    });
  }, [candidates]);

  const toggle = (key: string) => {
    setDeselected((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const selectedCandidates = useMemo(
    () => keyed.filter(({ key }) => !deselected.has(key)).map(({ candidate }) => candidate),
    [keyed, deselected],
  );

  const aiCandidateCount = candidates.length - ruleCandidateCount;
  const conflictingSelection = hasConflictingTranscriptEdits(selectedCandidates);
  const aiCount = aiRuns.length;
  const lastRun = aiRuns.length > 0 ? aiRuns[aiRuns.length - 1] : null;
  const canContinue = lastRun?.next_start_index != null && targetKey(lastRun.target) === targetKey(selectedTarget);
  /** 选中的模型是不是本地内置的：是就给"能力有限"的提示 */
  const selectedOption = modelOptions.find((option) =>
    option.target.kind === 'summary'
      ? selectedTarget.kind === 'summary'
      : selectedTarget.kind === 'provider' &&
        option.target.kind === 'provider' &&
        option.target.provider === selectedTarget.provider &&
        option.target.model === selectedTarget.model,
  );
  const selectedIsLocal = selectedOption?.is_local ?? false;
  /** 供应商显示名（沿用设置里那套叫法） */
  const providerLabel = (provider: string) => {
    const labels: Record<string, string> = {
      'builtin-ai': t('correction.providerBuiltin'),
      openai: 'OpenAI',
      claude: 'Claude',
      groq: 'Groq',
      openrouter: 'OpenRouter',
      ollama: 'Ollama',
      'custom-openai': t('correction.providerCustomOpenAi'),
    };
    return labels[provider] ?? provider;
  };
  /** 跑过本地内置模型：它抓不住明显的同音字错（实测），要给出 API 复查出口 */
  const hasCandidates = candidates.length > 0;

  return (
    <div className="space-y-3 py-2">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm">
        <Sparkles className="h-4 w-4 shrink-0 text-blue-600" aria-hidden="true" />
        {hasCandidates ? (
          <span className="font-medium text-gray-900">
            {t('correction.found', { count: candidates.length })}
          </span>
        ) : (
          <span className="text-gray-600">{t('correction.noCandidates')}</span>
        )}
        {hasCandidates && (
          <span className="text-xs text-gray-500">
            {aiCount > 0
              ? t('correction.sourceBreakdownAi', {
                  rules: ruleCandidateCount,
                  ai: aiCandidateCount,
                })
              : t('correction.sourceBreakdownRules', { rules: ruleCandidateCount })}
          </span>
        )}
        {appliedRules.length > 0 && (
          <span
            className="truncate text-xs text-gray-400"
            title={t('enhancementReview.correctionRules', { rules: appliedRules.join('、') })}
          >
            {t('correction.rulesHit', { count: appliedRules.length })}
          </span>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-2 text-xs text-gray-500">
        {aiRuns.length > 0 ? (
          aiRuns.map((run, index) => (
            <span key={`${run.model}-${index}`}>
              {index > 0 && ' · '}
              {t('correction.aiRange', {
                model: run.model,
                seconds: Math.max(1, Math.round(run.elapsed_ms / 1000)),
                start: run.start_index + 1,
                end: run.next_start_index ?? run.total_segments,
                total: run.total_segments,
              })}
              {run.missing_segments.length > 0
                ? ` ${t('correction.aiUnanswered', { count: run.missing_segments.length })}`
                : ''}
            </span>
          ))
        ) : (
          <span>{t('correction.aiNotRun')}</span>
        )}
        {canContinue && <span>· {t('proofread.remaining', { count: lastRun.total_segments - (lastRun.next_start_index ?? 0) })}</span>}
        {aiRuns.reduce((total, run) => total + run.warnings.length, 0) > 0 && (
          <span
            title={aiRuns.flatMap((run) => run.warnings).join('\n')}
          >
            · {t('correction.aiWarnings', { count: aiRuns.reduce((total, run) => total + run.warnings.length, 0) })}
          </span>
        )}
        {lastRun?.diagnostics_file && (
          <span title={lastRun.diagnostics_file}>· {t('correction.diagnosticsSaved')}</span>
        )}
      </div>

      {hasCandidates ? (
        <div className="max-h-[46vh] space-y-2 overflow-y-auto pr-1">
          {keyed.map(({ key, candidate }) => {
            const checked = !deselected.has(key);
            const labelKey = SOURCE_LABEL_KEYS[candidate.reason as SourceReason];
            return (
              <label
                key={key}
                className={`flex cursor-pointer gap-2 rounded-md border p-2 text-sm transition-colors ${
                  checked ? 'border-blue-200 bg-blue-50/40' : 'border-gray-200 bg-white'
                }`}
              >
                <input
                  type="checkbox"
                  className="mt-1 h-4 w-4 shrink-0 accent-blue-600"
                  checked={checked}
                  onChange={() => toggle(key)}
                />
                <div className="min-w-0 flex-1 space-y-1">
                  <div className="flex flex-wrap items-center gap-2 text-xs text-gray-500">
                    {/* 两个模型都跑过时，标出这条候选是谁提的（本地模型的误改一眼能认出来） */}
                    {aiCount > 1 && candidate.sourceKind && candidate.sourceKind !== 'rules' && (
                      <span
                        className={`rounded border px-1.5 py-0.5 text-[11px] font-medium ${SOURCE_KIND_STYLES[candidate.sourceKind]}`}
                      >
                        {t(SOURCE_KIND_LABEL_KEYS[candidate.sourceKind])}
                      </span>
                    )}
                    <span
                      className={`rounded border px-1.5 py-0.5 text-[11px] font-medium ${
                        SOURCE_STYLES[candidate.reason] ?? 'bg-gray-100 text-gray-600 border-gray-200'
                      }`}
                    >
                      {labelKey ? t(labelKey) : candidate.reason}
                    </span>
                    {!labelKey && (
                      <span
                        className={`rounded border px-1.5 py-0.5 text-[11px] font-medium ${
                          CONFIDENCE_STYLES[candidate.confidence] ?? CONFIDENCE_STYLES.medium
                        }`}
                      >
                        {candidate.confidence}
                      </span>
                    )}
                    <span>{formatSeconds(candidate.audio_start_time)}</span>
                  </div>
                  <p className="leading-relaxed text-gray-500">
                    <SnippetHighlight
                      text={candidate.segment_text}
                      snippet={candidate.original}
                      startChar={candidate.start_char}
                      tone="before"
                    />
                  </p>
                  <p className="leading-relaxed text-gray-900">
                    <SnippetHighlight
                      text={candidate.proposed_text}
                      snippet={candidate.suggested}
                      startChar={candidate.start_char}
                      tone="after"
                    />
                  </p>
                </div>
              </label>
            );
          })}
        </div>
      ) : (
        <div className="flex items-center gap-1 text-xs text-gray-500">
          {t('correction.contextInfo', { persons: contextPersonCount, terms: contextTermCount })}
          <HelpHint text={t('correction.noCandidatesHint')} />
        </div>
      )}

      <div className="space-y-2 border-t pt-3">
        {conflictingSelection && (
          <p className="text-xs text-amber-800" role="status">{t('proofread.conflictingEdits')}</p>
        )}
        {/*
          「校对用哪个模型」：把校对和摘要解耦，而且不限于某一个供应商 ——
          摘要模型 + 每一个配置好的 API（OpenAI/Claude/Groq/OpenRouter/Ollama/自定义兼容服务）都能选。
        */}
        {modelOptions.length > 1 && (
          <div className="flex flex-wrap items-center gap-2 text-xs text-gray-600">
            <span className="shrink-0">{t('correction.modelPickerLabel')}</span>
            <HelpHint><p>{t('correction.modelPickerHint')}</p>{selectedIsLocal && <p className="mt-2">{t('correction.localModelNote')}</p>}</HelpHint>
            <Select
              value={targetKey(selectedTarget)}
              onValueChange={(value) => {
                const option = modelOptions.find((entry) => targetKey(entry.target) === value);
                if (option) onTargetChange(option.target);
              }}
              disabled={isRunningAi || isApplying || isSavingModel}
            >
              <SelectTrigger className="h-7 w-full min-w-0 flex-1 text-xs sm:w-auto sm:max-w-[320px]">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {modelOptions.map((option) => (
                  <SelectItem key={targetKey(option.target)} value={targetKey(option.target)} className="text-xs">
                    {option.target.kind === 'summary'
                      ? t('correction.modelFollowSummary', { model: option.model })
                      : t('correction.modelProviderOption', {
                          provider: providerLabel(option.provider),
                          model: option.model,
                        })}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        )}
        <div className="flex flex-col-reverse gap-2 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex flex-wrap items-center gap-2">
            <Button
              type="button"
              size="sm"
              variant="outline"
              className="h-8 text-xs"
              onClick={onRunAi}
              disabled={isRunningAi || isApplying || isSavingModel}
            >
              {isRunningAi ? (
                <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" aria-hidden="true" />
              ) : (
                <Wand2 className="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
              )}
              {isRunningAi
                ? t('correction.runAiRunning')
                : canContinue
                  ? t('correction.continueAi', { start: (lastRun.next_start_index ?? 0) + 1 })
                : aiCount > 0
                  ? t('correction.runAiAgain')
                  : t('correction.runAi')}
            </Button>
            <HelpHint text={t('correction.aiHint')} />
          </div>
          <div className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
            <Button type="button" variant="outline" onClick={onCancel} disabled={isApplying || isRunningAi || isSavingModel}>
              <X className="mr-2 h-4 w-4" aria-hidden="true" />
              {t('proofread.cancel')}
            </Button>
            <Button
              type="button"
              onClick={() => onApply(selectedCandidates)}
              disabled={isApplying || isRunningAi || isSavingModel || conflictingSelection || selectedCandidates.length === 0}
            >
              {isApplying ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" aria-hidden="true" />
              ) : (
                <Check className="mr-2 h-4 w-4" aria-hidden="true" />
              )}
              {isApplying
                ? t('proofread.applying')
                : t('proofread.apply', { count: selectedCandidates.length })}
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
