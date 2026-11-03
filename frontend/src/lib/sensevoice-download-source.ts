export const SENSEVOICE_DOWNLOAD_SOURCES = [
  { id: 'auto', label: '自动切换（推荐）', description: '依次尝试 Hugging Face、ModelScope、HF 镜像站。' },
  { id: 'huggingface', label: 'Hugging Face', description: 'huggingface.co · 固定版本的 ONNX 模型' },
  { id: 'modelscope', label: 'ModelScope（国内）', description: 'modelscope.cn · 社区镜像' },
  { id: 'hf-mirror', label: 'HF 镜像站（国内）', description: 'hf-mirror.com · 第三方镜像' },
] as const;

export type SenseVoiceDownloadSource = typeof SENSEVOICE_DOWNLOAD_SOURCES[number]['id'];
const STORAGE_KEY = 'sensevoice-download-source';

export function getSenseVoiceDownloadSource(): SenseVoiceDownloadSource {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    return SENSEVOICE_DOWNLOAD_SOURCES.find(source => source.id === stored)?.id ?? 'auto';
  } catch { return 'auto'; }
}

export function saveSenseVoiceDownloadSource(source: SenseVoiceDownloadSource) {
  try { localStorage.setItem(STORAGE_KEY, source); } catch { /* Storage may be unavailable. */ }
}
