import assert from "node:assert/strict";
import test from "node:test";
import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import TestRenderer, { act } from "react-test-renderer";
import {
  MeetilyI18nProvider,
  useMeetilyI18n,
} from "../../src/i18n/I18nProvider";
import { UI_LOCALE_STORAGE_KEY } from "../../src/i18n/types";

class MemoryStorage {
  readonly values = new Map<string, string>();

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }
}

test("provider switches immediately, persists across remounts, and preserves child state", async () => {
  const storage = new MemoryStorage();
  storage.setItem(UI_LOCALE_STORAGE_KEY, "en");
  storage.setItem("primaryLanguage", "ja");
  storage.setItem("summaryLanguage", "de");

  const listeners = new Map<string, Set<EventListener>>();
  const fakeWindow = {
    localStorage: storage,
    navigator: { language: "en-US", languages: ["en-US"] },
    addEventListener(type: string, listener: EventListener) {
      const handlers = listeners.get(type) ?? new Set<EventListener>();
      handlers.add(listener);
      listeners.set(type, handlers);
    },
    removeEventListener(type: string, listener: EventListener) {
      listeners.get(type)?.delete(listener);
    },
  };
  const documentElement = { lang: "en", dir: "ltr" };

  const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  const originalDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const originalNavigator = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: fakeWindow,
  });
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: { documentElement },
  });
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: fakeWindow.navigator,
  });

  function Probe() {
    const { t } = useTranslation("settings");
    const { locale, preference, setPreference } = useMeetilyI18n();
    const [counter, setCounter] = useState(0);

    return (
      <div data-testid="probe">
        <span data-testid="title">{t("displayLanguage.title")}</span>
        <span data-testid="fallback">{t("labels.dataStorageLocations")}</span>
        <span data-testid="locale">{locale}</span>
        <span data-testid="preference">{preference}</span>
        <span data-testid="counter">{counter}</span>
        <button data-testid="increment" onClick={() => setCounter((value) => value + 1)} />
        <button data-testid="chinese" onClick={() => void setPreference("zh-CN")} />
      </div>
    );
  }

  const textOf = (renderer: TestRenderer.ReactTestRenderer, testId: string) =>
    renderer.root.findByProps({ "data-testid": testId }).children.join("");

  try {
    const renderer = TestRenderer.create(
      <MeetilyI18nProvider>
        <Probe />
      </MeetilyI18nProvider>,
    );

    const initialGate = renderer.root.findByProps({ "data-i18n-ready": "false" });
    assert.equal(initialGate.props.style.visibility, "hidden");

    await act(async () => {
      await Promise.resolve();
    });

    assert.equal(textOf(renderer, "title"), "Display language");
    assert.equal(textOf(renderer, "locale"), "en");

    act(() => {
      renderer.root.findByProps({ "data-testid": "increment" }).props.onClick();
    });
    assert.equal(textOf(renderer, "counter"), "1");

    await act(async () => {
      renderer.root.findByProps({ "data-testid": "chinese" }).props.onClick();
      await Promise.resolve();
    });

    assert.equal(textOf(renderer, "title"), "显示语言");
    assert.equal(textOf(renderer, "fallback"), "数据存储位置");
    assert.equal(textOf(renderer, "locale"), "zh-CN");
    assert.equal(textOf(renderer, "preference"), "zh-CN");
    assert.equal(textOf(renderer, "counter"), "1");
    assert.equal(storage.getItem(UI_LOCALE_STORAGE_KEY), "zh-CN");
    assert.equal(storage.getItem("primaryLanguage"), "ja");
    assert.equal(storage.getItem("summaryLanguage"), "de");
    assert.deepEqual(documentElement, { lang: "zh-CN", dir: "ltr" });

    act(() => {
      renderer.unmount();
    });

    let remounted: TestRenderer.ReactTestRenderer;
    await act(async () => {
      remounted = TestRenderer.create(
        <MeetilyI18nProvider>
          <Probe />
        </MeetilyI18nProvider>,
      );
      await Promise.resolve();
    });

    assert.equal(textOf(remounted!, "title"), "显示语言");
    assert.equal(textOf(remounted!, "preference"), "zh-CN");
    assert.equal(
      remounted!.root.findByProps({ "data-i18n-ready": "true" }).props.style.visibility,
      "visible",
    );
    act(() => {
      remounted!.unmount();
    });
  } finally {
    if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
    else Reflect.deleteProperty(globalThis, "window");
    if (originalDocument) Object.defineProperty(globalThis, "document", originalDocument);
    else Reflect.deleteProperty(globalThis, "document");
    if (originalNavigator) Object.defineProperty(globalThis, "navigator", originalNavigator);
    else Reflect.deleteProperty(globalThis, "navigator");
  }
});
