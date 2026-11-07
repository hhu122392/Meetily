'use client';

import { useCallback, useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { FileText, Sparkles, Columns2 } from 'lucide-react';
import { companyStorageKey } from '@/lib/company-scope';
import { t, useUiLanguage } from "@/i18n";
const STORAGE_KEY = 'meetily.meetingDetails.transcriptPaneRatio';
const DEFAULT_RATIO = 0.3;
const MIN_RATIO = 0.3;
const MAX_RATIO = 0.5;

const TABS = [
  { value: 'summary' as const, label: 'AI 摘要', icon: Sparkles },
  { value: 'transcript' as const, label: '转写', icon: FileText },
  { value: 'split' as const, label: '并排', icon: Columns2 },
];

function readStoredRatio(): number {
  if (typeof window === 'undefined') return DEFAULT_RATIO;
  try {
    const raw = localStorage.getItem(companyStorageKey(STORAGE_KEY));
    const n = raw == null ? NaN : Number(raw);
    return Number.isFinite(n) && n >= MIN_RATIO && n <= MAX_RATIO ? n : DEFAULT_RATIO;
  } catch {
    return DEFAULT_RATIO;
  }
}

function writeStoredRatio(value: number): void {
  try {
    localStorage.setItem(companyStorageKey(STORAGE_KEY), String(value));
  } catch {
    // Layout persistence is optional.
  }
}

export type MeetingDetailsTab = 'transcript' | 'summary' | 'split';

interface MeetingDetailsSplitViewProps {
  transcript: ReactNode;
  summary: ReactNode;
  activeTab: MeetingDetailsTab;
  onTabChange: (tab: MeetingDetailsTab) => void;
}

export function MeetingDetailsSplitView({
  transcript,
  summary,
  activeTab,
  onTabChange,
}: MeetingDetailsSplitViewProps) {
  useUiLanguage();
  const panelId = useId();
  const containerRef = useRef<HTMLDivElement>(null);
  const [ratio, setRatio] = useState(DEFAULT_RATIO);
  const [isDesktop, setIsDesktop] = useState(true);
  const dragging = useRef(false);
  const clampRatio = (value: number) => Math.min(MAX_RATIO, Math.max(MIN_RATIO, value));

  useEffect(() => {
    setRatio(readStoredRatio());
  }, []);

  useEffect(() => {
    const mediaQuery = window.matchMedia('(min-width: 768px)');
    const updateLayout = () => setIsDesktop(mediaQuery.matches);
    updateLayout();
    mediaQuery.addEventListener('change', updateLayout);
    return () => mediaQuery.removeEventListener('change', updateLayout);
  }, []);

  const onPointerDown = useCallback((event: React.PointerEvent) => {
    event.preventDefault();
    dragging.current = true;
    event.currentTarget.setPointerCapture(event.pointerId);
  }, []);

  const onPointerMove = useCallback((event: React.PointerEvent) => {
    if (!dragging.current || !containerRef.current) return;
    const rect = containerRef.current.getBoundingClientRect();
    setRatio(clampRatio((event.clientX - rect.left) / rect.width));
  }, []);

  const onPointerUp = useCallback(() => {
    if (!dragging.current) return;
    dragging.current = false;
    setRatio((current) => {
      writeStoredRatio(current);
      return current;
    });
  }, []);

  const onSeparatorKeyDown = useCallback((event: React.KeyboardEvent) => {
    const current = ratio;
    const next =
      event.key === 'ArrowLeft' ? clampRatio(current - 0.05) :
      event.key === 'ArrowRight' ? clampRatio(current + 0.05) :
      event.key === 'Home' ? MIN_RATIO :
      event.key === 'End' ? MAX_RATIO :
      null;
    if (next === null) return;
    event.preventDefault();
    setRatio(next);
    writeStoredRatio(next);
  }, [ratio]);

  const showTranscript = activeTab === 'transcript' || (activeTab === 'split' && isDesktop);
  const showSummary = activeTab === 'summary' || activeTab === 'split';
  const split = showTranscript && showSummary;
  return <div className="flex flex-1 min-h-0 min-w-0 flex-col overflow-hidden">
    <div className="flex shrink-0 justify-center border-b bg-white px-3 py-2">
      <div role="group" aria-label="会议视图" className="inline-flex items-center gap-1 rounded-full border border-gray-200 bg-gray-100/90 p-1 shadow-sm backdrop-blur">
        {TABS.map(({value,label,icon:Icon}) => <button key={value} type="button" aria-pressed={activeTab===value}
          aria-controls={value==='split' ? panelId+'-transcript '+panelId+'-summary' : panelId+'-'+value}
          className={'flex items-center gap-1.5 rounded-full px-3 py-1.5 text-sm font-medium transition-colors focus-visible:outline-blue-600 '+(activeTab===value?'bg-white text-gray-900 shadow-sm':'text-gray-500 hover:text-gray-700')}
          onClick={()=>onTabChange(value)}><Icon size={16}/>{t(label)}</button>)}
      </div>
    </div>
    {activeTab==='split'&&!isDesktop&&<p className="bg-gray-50 px-3 py-1 text-center text-xs text-gray-500">窗口较窄，当前显示纪要；可切换转写视图。</p>}
    <div ref={containerRef} className="flex flex-1 min-h-0 min-w-0" onPointerMove={onPointerMove} onPointerUp={onPointerUp} onPointerCancel={onPointerUp}>
      <div id={panelId+'-transcript'} role="region" aria-label={t('Transcript')} aria-hidden={!showTranscript}
        className="min-h-0 min-w-0 flex-col overflow-hidden" style={{display:showTranscript?'flex':'none',width:split ? ratio*100+'%' : '100%'}}>{transcript}</div>
      {split&&<div role="separator" aria-orientation="vertical" aria-valuenow={Math.round(ratio*100)} aria-valuemin={MIN_RATIO*100} aria-valuemax={MAX_RATIO*100}
        aria-label={t('Resize transcript and summary')} tabIndex={0} onPointerDown={onPointerDown} onKeyDown={onSeparatorKeyDown}
        className="group relative z-10 flex w-2 shrink-0 cursor-col-resize justify-center focus-visible:outline-blue-600">
        <div className="h-full w-px bg-gray-200 group-hover:w-1 group-hover:bg-blue-400"/>
      </div>}
      <div id={panelId+'-summary'} role="region" aria-label={t('Summary')} aria-hidden={!showSummary}
        className="min-h-0 min-w-0 flex-1 flex-col overflow-hidden" style={{display:showSummary?'flex':'none'}}>{summary}</div>
    </div>
  </div>;
}
