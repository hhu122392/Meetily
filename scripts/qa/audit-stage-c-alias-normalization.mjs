import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const [templatePath, transcriptPath, reportPath] = process.argv.slice(2);
if (!templatePath || !transcriptPath || !reportPath) {
  throw new Error(
    "Usage: node audit-stage-c-alias-normalization.mjs <template.json> <transcripts.json> <report.json>",
  );
}

const templateBytes = fs.readFileSync(templatePath);
const transcriptBytes = fs.readFileSync(transcriptPath);
const template = JSON.parse(templateBytes.toString("utf8"));
const transcript = JSON.parse(transcriptBytes.toString("utf8"));
const profile = template.extensions?.meetily_meeting_context;
if (!profile) throw new Error("Template does not contain meetily_meeting_context");
if (!Array.isArray(transcript.segments)) throw new Error("Transcript does not contain segments");

const replacements = [];
for (const person of profile.people.filter((item) => item.enabled !== false)) {
  for (const alias of person.aliases) replacements.push([alias, person.display_name, "person"]);
}
for (const term of profile.terms.filter((item) => item.enabled !== false)) {
  for (const alias of term.aliases) replacements.push([alias, term.canonical, "term"]);
}
replacements.sort(([left], [right]) => (
  [...right].length - [...left].length || (left < right ? -1 : left > right ? 1 : 0)
));

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function aliasRegExp(alias, global = true) {
  const escaped = escapeRegExp(alias);
  if (/^[\x00-\x7F]+$/.test(alias)) {
    return new RegExp(`(?<![A-Za-z0-9_])${escaped}(?![A-Za-z0-9_])`, global ? "gi" : "i");
  }
  return new RegExp(escaped, global ? "g" : "");
}

function countMatches(text, alias) {
  return [...text.matchAll(aliasRegExp(alias))].length;
}

function normalize(text) {
  return replacements.reduce(
    (current, [alias, canonical]) => current.replace(aliasRegExp(alias), canonical),
    text,
  );
}

const originalText = transcript.segments.map((segment) => segment.text).join("\n");
const normalizedSegments = transcript.segments.map((segment) => ({
  sequenceId: segment.sequence_id,
  original: segment.text,
  normalized: normalize(segment.text),
}));
const normalizedText = normalizedSegments.map((segment) => segment.normalized).join("\n");
const configuredAliases = replacements.map(([alias, canonical, kind]) => ({
  alias,
  canonical,
  kind,
  originalOccurrences: countMatches(originalText, alias),
  remainingAliasOccurrences: countMatches(normalizedText, alias),
  changedSegments: normalizedSegments.filter((segment) => segment.original !== segment.normalized
    && countMatches(segment.original, alias) > 0).map((segment) => segment.sequenceId),
}));

const deliberatelyUnmappedTokens = ["老学", "小比", "思域", "黑炭", "阿姨", "粉泥", "我牛"];
const unmappedAssertions = deliberatelyUnmappedTokens.map((token) => ({
  token,
  originalOccurrences: originalText.split(token).length - 1,
  normalizedOccurrences: normalizedText.split(token).length - 1,
}));

const changedSegments = normalizedSegments
  .filter((segment) => segment.original !== segment.normalized)
  .map((segment) => ({
    sequenceId: segment.sequenceId,
    before: segment.original,
    after: segment.normalized,
  }));
const assertions = {
  templateIsLicenseStationV3: template.id === "license_station_weekly" && template.version === 3,
  configuredAliasesPresentInFixture: configuredAliases.some((entry) => entry.originalOccurrences > 0),
  everyObservedConfiguredAliasReplaced: configuredAliases.every((entry) => (
    entry.originalOccurrences === 0 || entry.remainingAliasOccurrences === 0
  )),
  deliberatelyUnmappedTokensUnchanged: unmappedAssertions.every((entry) => (
    entry.originalOccurrences === entry.normalizedOccurrences
  )),
  onlyConfiguredAliasSegmentsChanged: changedSegments.every((segment) => replacements.some(
    ([alias]) => countMatches(segment.before, alias) > 0,
  )),
  sourceTranscriptNotModified: crypto.createHash("sha256").update(fs.readFileSync(transcriptPath)).digest("hex")
    === crypto.createHash("sha256").update(transcriptBytes).digest("hex"),
};
const failed = Object.entries(assertions).filter(([, passed]) => !passed).map(([name]) => name);
if (failed.length > 0) throw new Error(`Stage C alias audit failed: ${failed.join(", ")}`);

const report = {
  stage: "C",
  gate: "MC-R01 deterministic alias normalization",
  auditedAt: new Date().toISOString(),
  template: {
    path: path.resolve(templatePath),
    id: template.id,
    version: template.version,
    sha256: crypto.createHash("sha256").update(templateBytes).digest("hex"),
  },
  transcript: {
    path: path.resolve(transcriptPath),
    totalSegments: transcript.segments.length,
    sha256: crypto.createHash("sha256").update(transcriptBytes).digest("hex"),
  },
  configuredAliases,
  deliberatelyUnmappedTokens: unmappedAssertions,
  changedSegmentCount: changedSegments.length,
  changedSegments,
  assertions,
  scopeNote: "This proves deterministic post-transcription normalization on a real saved transcript. It does not claim that the acoustic model recognized unconfigured names correctly.",
  result: "PASS",
};
fs.mkdirSync(path.dirname(reportPath), { recursive: true });
fs.writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");
console.log(JSON.stringify(report, null, 2));
