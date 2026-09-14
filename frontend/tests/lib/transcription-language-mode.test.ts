import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const read = (path: string) => readFileSync(new URL(`../../${path}`, import.meta.url), 'utf8');

test('live transcription defaults to original-language detection instead of English translation', () => {
  const backend = read('src-tauri/src/lib.rs');

  assert.match(backend, /DEFAULT_TRANSCRIPTION_LANGUAGE:\s*&str\s*=\s*"auto"/);
  assert.doesNotMatch(backend, /LazyLock::new\(\|\|\s*StdMutex::new\("auto-translate"/);
});

test('Chinese aliases select Whisper zh without enabling the English translation task', () => {
  const whisper = read('src-tauri/src/whisper_engine/whisper_engine.rs');

  assert.match(whisper, /"zh-cn"\s*\|\s*"zh-sg"\s*\|\s*"zh-hans"/);
  assert.match(
    whisper,
    /language_code:\s*Some\("zh"\.to_string\(\)\),\s*translate_to_english:\s*false/,
  );
  assert.match(
    whisper,
    /"auto-translate"\s*=>[\s\S]*?language_code:\s*None,[\s\S]*?translate_to_english:\s*true/,
  );
});

test('the settings UI waits for Rust acknowledgement and explains source versus target language', () => {
  const context = read('src/contexts/ConfigContext.tsx');
  const selector = read('src/components/LanguageSelection.tsx');
  const english = JSON.parse(read('src/i18n/locales/en/transcription.json'));
  const chinese = JSON.parse(read('src/i18n/locales/zh-CN/transcription.json'));

  assert.match(
    context,
    /await syncLanguagePreference\(compatibleLanguage\);[\s\S]*?setSelectedLanguage\(compatibleLanguage\)/,
  );
  assert.match(selector, /await onLanguageChange\(languageCode\)/);
  assert.doesNotMatch(selector, /useConfig\(\)/);
  assert.match(
    english.descriptions.transcriptionLanguageIsSourceNotTranslationTarget,
    /not a translation target/i,
  );
  assert.match(
    chinese.descriptions.transcriptionLanguageIsSourceNotTranslationTarget,
    /不是翻译目标语言/,
  );
});

test('Parakeet is automatic-only and rejects Chinese or Whisper translation mode', () => {
  const provider = read('src-tauri/src/audio/transcription/parakeet_provider.rs');
  const worker = read('src-tauri/src/audio/transcription/worker.rs');
  const importAudio = read('src-tauri/src/audio/import.rs');
  const retranscription = read('src-tauri/src/audio/retranscription.rs');
  const english = JSON.parse(read('src/i18n/locales/en/transcription.json'));
  const chinese = JSON.parse(read('src/i18n/locales/zh-CN/transcription.json'));

  assert.match(provider, /validate_parakeet_language\(language\.as_deref\(\)\)\?/);
  assert.match(worker, /validate_parakeet_language\(language\.as_deref\(\)\)/);
  assert.equal(
    importAudio.match(/validate_parakeet_language\([\s\S]*?language\.as_deref\(\)/g)?.length,
    2,
  );
  assert.equal(
    retranscription.match(/validate_parakeet_language\([\s\S]*?language\.as_deref\(\)/g)?.length,
    2,
  );
  assert.match(
    english.descriptions.parakeetTdtV3LanguageLimitations,
    /does not support Chinese/i,
  );
  assert.match(
    chinese.descriptions.parakeetTdtV3LanguageLimitations,
    /不支持中文/,
  );
});
