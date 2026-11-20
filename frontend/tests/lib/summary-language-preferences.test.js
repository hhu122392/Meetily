import { beforeEach, describe, expect, mock, test } from "bun:test";
import { initializeCompanyScope } from '../../src/lib/company-scope';
initializeCompanyScope('legacy');

const invokeMock = mock(async () => null);

mock.module("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

function installLocalStorage() {
  const values = new Map();

  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      localStorage: {
        getItem: (key) => values.get(key) ?? null,
        setItem: (key, value) => {
          values.set(key, value);
        },
        removeItem: (key) => {
          values.delete(key);
        },
        clear: () => {
          values.clear();
        },
      },
    },
  });

  return values;
}

function installFailingLocalStorage() {
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      localStorage: {
        getItem: () => null,
        setItem: () => {
          throw new Error("quota exceeded");
        },
        removeItem: () => {},
        clear: () => {},
      },
    },
  });
}

describe("summary language local fallback", () => {
  let storageValues;

  beforeEach(() => {
    invokeMock.mockReset();
    storageValues = installLocalStorage();
  });

  test("新安装默认中文，显式选择自动或其他语言后重读不覆盖", async () => {
    const prefs = await import("../../src/lib/summary-language-preferences");
    expect(prefs.readPinnedSummaryLanguageDefault()).toBe("zh");
    prefs.writePinnedSummaryLanguageDefault(null);
    expect(prefs.readPinnedSummaryLanguageDefault()).toBeNull();
    expect(storageValues.get(prefs.SUMMARY_LANGUAGE_DEFAULT_KEY)).toBe("auto");
    prefs.writePinnedSummaryLanguageDefault("en");
    expect(prefs.readPinnedSummaryLanguageDefault()).toBe("en");
  });

  test("新会议保存默认中文，取消默认后不再强制语言", async () => {
    const prefs = await import("../../src/lib/summary-language-preferences");
    invokeMock.mockResolvedValue({language: "zh", storage: "metadata"});
    expect(await prefs.applyPinnedSummaryLanguageToMeeting("new-meeting")).toBe("zh");
    expect(invokeMock).toHaveBeenCalledWith("api_save_meeting_summary_language", {meetingId: "new-meeting", summaryLanguage: "zh"});
    invokeMock.mockClear();
    prefs.writePinnedSummaryLanguageDefault(null);
    expect(await prefs.applyPinnedSummaryLanguageToMeeting("auto-meeting")).toBeNull();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  test("reads summary language from local fallback when meeting has no folder", async () => {
    const prefs = await import("../../src/lib/summary-language-preferences");
    storageValues.set("summaryLanguageFallback:meeting-1", "fr");
    invokeMock.mockResolvedValueOnce({
      language: null,
      storage: "local_fallback",
    });

    await expect(prefs.readMeetingSummaryLanguage("meeting-1")).resolves.toEqual({
      language: "fr",
      storage: "local_fallback",
    });
  });

  test("saves summary language locally when command reports no folder", async () => {
    const prefs = await import("../../src/lib/summary-language-preferences");
    invokeMock.mockResolvedValueOnce({
      language: null,
      storage: "local_fallback",
    });

    await expect(
      prefs.saveMeetingSummaryLanguage("meeting-1", "es"),
    ).resolves.toEqual({
      language: "es",
      storage: "local_fallback",
    });

    expect(storageValues.get("summaryLanguageFallback:meeting-1")).toBe("es");
  });

  test("clears local fallback when Auto is saved for a folderless meeting", async () => {
    const prefs = await import("../../src/lib/summary-language-preferences");
    storageValues.set("summaryLanguageFallback:meeting-1", "de");
    invokeMock.mockResolvedValueOnce({
      language: null,
      storage: "local_fallback",
    });

    await expect(
      prefs.saveMeetingSummaryLanguage("meeting-1", null),
    ).resolves.toEqual({
      language: null,
      storage: "local_fallback",
    });

    expect(storageValues.has("summaryLanguageFallback:meeting-1")).toBe(false);
  });

  test("caches detected language locally when meeting has no folder", async () => {
    const prefs = await import("../../src/lib/summary-language-preferences");
    invokeMock.mockResolvedValueOnce({
      language: null,
      storage: "local_fallback",
    });

    await prefs.saveCachedDetectedSummaryLanguage("meeting-1", "pt");

    expect(storageValues.get("detectedSummaryLanguageFallback:meeting-1")).toBe("pt");
  });

  test("rejects when folderless summary language cannot be persisted locally", async () => {
    const prefs = await import("../../src/lib/summary-language-preferences");
    installFailingLocalStorage();
    invokeMock.mockResolvedValueOnce({
      language: null,
      storage: "local_fallback",
    });

    await expect(
      prefs.saveMeetingSummaryLanguage("meeting-1", "it"),
    ).rejects.toThrow("Failed to save summary language on this device");
  });
});
