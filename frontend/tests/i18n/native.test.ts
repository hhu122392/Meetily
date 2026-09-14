import assert from "node:assert/strict";
import test from "node:test";

import {
  getNativeUiLocale,
  isNativeErrorPayload,
  nativeErrorTranslationKey,
  syncNativeUiLocale,
} from "../../src/lib/native-i18n";

test("native error payloads require the complete stable schema", () => {
  assert.equal(
    isNativeErrorPayload({
      code: "NOTIFICATION_OPERATION_FAILED",
      params: {},
      debugMessage: "Notification operation failed",
    }),
    true,
  );
  assert.equal(isNativeErrorPayload(new Error("legacy string error")), false);
  assert.equal(isNativeErrorPayload({ code: "NATIVE_UNKNOWN" }), false);
});

test("known and unknown native errors resolve to a safe translation key", () => {
  assert.equal(
    nativeErrorTranslationKey({
      code: "NOTIFICATION_MANAGER_UNAVAILABLE",
      params: {},
      debugMessage: "Notification manager was not initialized",
    }),
    "common:errors.unknown",
  );
  assert.equal(
    nativeErrorTranslationKey({
      code: "UNREGISTERED_CODE",
      params: {},
      debugMessage: "Unknown",
    }),
    "common:errors.unknown",
  );
  assert.equal(nativeErrorTranslationKey("raw native error"), "common:errors.unknown");
});

test("native locale bridge is a no-op in browser-independent rendering", async () => {
  assert.equal(await getNativeUiLocale(), null);
  assert.equal(await syncNativeUiLocale("zh-CN", "zh-CN"), null);
});
