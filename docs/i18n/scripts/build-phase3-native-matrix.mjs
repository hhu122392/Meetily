#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

const root = path.resolve(process.argv[2] || ".");
const catalogPath = path.join(root, "target/release/docs/en.catalog.json");
const catalog = JSON.parse(await fs.readFile(catalogPath, "utf8"));
const nativeEntries = catalog.entries.filter((entry) => entry.layer === "tauri");

function classify(entry) {
  if (entry.status === "excluded_legacy_source") {
    return {
      disposition: "excluded_legacy_source",
      runtimeVisibility: "excluded",
      action: "Keep outside the Rust module graph; audit that no mod/include reference exists.",
      rationale: "The catalog marked this candidate as legacy source before Stage 3.",
    };
  }

  const files = entry.sources.map((source) => source.file);
  const joined = files.join(" ");
  if (/\/tray\.rs$/.test(joined)) {
    return {
      disposition: "native_locale_resource",
      runtimeVisibility: "user_visible",
      action: "Replace with a stable native locale key and rebuild the tray on locale change.",
      rationale: "The value is rendered by the native tray menu.",
    };
  }
  if (/\/notifications\/(types|commands|manager|system)\.rs$/.test(joined)) {
    return {
      disposition: "native_notification_or_error",
      runtimeVisibility: "user_visible_or_command_boundary",
      action: "Use native locale resources for notification copy and a stable error code at command boundaries.",
      rationale: "The value can reach an OS notification or a frontend invoke rejection.",
    };
  }

  const text = entry.en;
  if (
    /(?:failed|error|not initialized|not found|invalid|unable|cannot|could not|permission|denied|timed out|timeout|unsupported)/i.test(
      text,
    )
  ) {
    return {
      disposition: "structured_error_registry",
      runtimeVisibility: "reviewed_command_boundary",
      action: "Map to a stable NativeError code before exposing it; retain raw detail in logs only.",
      rationale: "Error-like native text may cross a Tauri command boundary.",
    };
  }

  if (
    /(?:^https?:|^[a-z0-9_.-]+$|session_|\.json$|\.wav$|SELECT |INSERT |UPDATE |DELETE )/i.test(
      text,
    )
  ) {
    return {
      disposition: "machine_or_storage_value",
      runtimeVisibility: "not_translatable",
      action: "Keep stable and untranslated.",
      rationale: "The value is a machine identifier, storage value, path fragment, URL, or query fragment.",
    };
  }

  return {
    disposition: "diagnostic_or_internal_reviewed",
    runtimeVisibility: "not_directly_rendered",
    action: "Keep English in logs/internal flow; localize only through NativeError if later exposed.",
    rationale: "No direct tray/notification renderer was found; the catalog source remains covered by the error boundary policy.",
  };
}

const entries = nativeEntries.map((entry) => ({
  id: entry.id,
  key: entry.key,
  english: entry.en,
  originalStatus: entry.status,
  sources: entry.sources,
  ...classify(entry),
}));

const counts = {};
for (const entry of entries) counts[entry.disposition] = (counts[entry.disposition] || 0) + 1;
const output = {
  schemaVersion: 1,
  phase: "15.6-stage-3-tauri-native-i18n",
  generatedAt: new Date().toISOString(),
  sourceCatalog: "target/release/docs/en.catalog.json",
  invariants: {
    expectedActiveCandidates: 447,
    expectedLegacyCandidates: 45,
    expectedTotalCandidates: 492,
  },
  summary: {
    total: entries.length,
    active: entries.filter((entry) => entry.originalStatus === "manual_visibility_review").length,
    excludedLegacy: entries.filter((entry) => entry.originalStatus === "excluded_legacy_source").length,
    unresolved: entries.filter((entry) => !entry.disposition).length,
    byDisposition: counts,
  },
  entries,
};

if (
  output.summary.total !== output.invariants.expectedTotalCandidates ||
  output.summary.active !== output.invariants.expectedActiveCandidates ||
  output.summary.excludedLegacy !== output.invariants.expectedLegacyCandidates ||
  output.summary.unresolved !== 0
) {
  throw new Error(`Stage 3 candidate invariant failed: ${JSON.stringify(output.summary)}`);
}

const outputPath = path.join(root, "docs/i18n/phase-3/native-candidate-disposition.json");
await fs.mkdir(path.dirname(outputPath), { recursive: true });
await fs.writeFile(outputPath, JSON.stringify(output, null, 2) + "\n");
process.stdout.write(JSON.stringify({ outputPath, ...output.summary }) + "\n");
