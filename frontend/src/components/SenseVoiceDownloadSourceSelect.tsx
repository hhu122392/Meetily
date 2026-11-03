import { SENSEVOICE_DOWNLOAD_SOURCES, type SenseVoiceDownloadSource } from '@/lib/sensevoice-download-source';

export function SenseVoiceDownloadSourceSelect({ value, onChange, disabled = false }: {
  value: SenseVoiceDownloadSource;
  onChange: (value: SenseVoiceDownloadSource) => void;
  disabled?: boolean;
}) {
  return <div className="space-y-2 rounded-lg border border-gray-200 bg-white p-3">
    <label className="flex items-center gap-3 text-sm font-medium">
      下载来源
      <select aria-label="SenseVoice 下载来源" value={value} disabled={disabled}
        onChange={event => onChange(event.target.value as SenseVoiceDownloadSource)}
        className="min-w-0 flex-1 rounded-md border border-gray-300 bg-white px-3 py-2 disabled:opacity-50">
        {SENSEVOICE_DOWNLOAD_SOURCES.map(source => <option key={source.id} value={source.id}>{source.label}</option>)}
      </select>
    </label>
    <p className="text-xs text-gray-500">{SENSEVOICE_DOWNLOAD_SOURCES.find(source => source.id === value)?.description} 各来源使用同一模型，并校验 SHA-256。</p>
    {disabled && <p className="text-xs text-gray-500">更换来源前请先取消当前下载。</p>}
  </div>;
}
