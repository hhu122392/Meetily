export function normalizeTranscriptionLanguage(provider: string | undefined, language: string | undefined): string {
  const value = language || 'auto';
  if (provider === 'sensevoice') return ['auto', 'zh', 'en', 'yue', 'ja', 'ko'].includes(value) ? value : 'auto';
  if (provider === 'parakeet') return 'auto';
  if (provider && value === 'yue') return 'auto';
  return value;
}
