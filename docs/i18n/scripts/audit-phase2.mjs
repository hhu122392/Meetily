#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

const root = path.resolve(process.argv[2] || ".");
const options = new Map(process.argv.slice(3).map((item) => {
  const parts = item.replace(/^--/, "").split("=");
  return [parts.shift(), parts.join("=") || "true"];
}));
const batchId = options.get("batch") || "2A";
const reportPath = path.resolve(
  root,
  options.get("report") ||
    "docs/i18n/audit/phase-2-react/" + batchId + "/resource-and-source-audit.json",
);
const manifest = JSON.parse(
  await fs.readFile(path.join(root, "docs/i18n/phase-2/phase2-batches.json"), "utf8"),
);
const allowlist = JSON.parse(
  await fs.readFile(path.join(root, "docs/i18n/phase-2/untranslated-allowlist.json"), "utf8"),
);
const batch = manifest.batches[batchId];
if (!batch) throw new Error("Unknown phase 2 batch: " + batchId);

const typescriptPath = path.join(
  root,
  "frontend/node_modules/typescript/lib/typescript.js",
);
const ts = (await import(pathToFileURL(typescriptPath).href)).default;

function flatten(value, prefix = "", result = new Map()) {
  for (const [key, child] of Object.entries(value)) {
    const next = prefix ? prefix + "." + key : key;
    if (child !== null && typeof child === "object" && !Array.isArray(child)) {
      flatten(child, next, result);
    } else {
      result.set(next, child);
    }
  }
  return result;
}

function placeholders(value) {
  if (typeof value !== "string") return [];
  return [...value.matchAll(/{{\s*([^},\s]+)[^}]*}}/g)]
    .map((match) => match[1])
    .sort();
}

function sourcePosition(sourceFile, node) {
  const position = sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile));
  return { line: position.line + 1, column: position.character + 1 };
}

function literalText(node) {
  if (
    ts.isStringLiteral(node) ||
    ts.isNoSubstitutionTemplateLiteral(node) ||
    ts.isJsxText(node)
  ) {
    return node.text.trim();
  }
  return "";
}

function isInsideJsxExpression(node) {
  for (let current = node.parent; current; current = current.parent) {
    if (ts.isJsxExpression(current)) return true;
    if (ts.isSourceFile(current) || ts.isBlock(current)) return false;
  }
  return false;
}

function enclosingJsxAttribute(node) {
  for (let current = node.parent; current; current = current.parent) {
    if (ts.isJsxAttribute(current)) return current;
    if (ts.isJsxElement(current) || ts.isJsxSelfClosingElement(current)) return null;
  }
  return null;
}

function propertyName(node) {
  if (!node?.name) return "";
  return ts.isIdentifier(node.name) || ts.isStringLiteral(node.name)
    ? node.name.text
    : "";
}

function findingKind(node, text) {
  if (!/[A-Za-z]{2}/.test(text) || /^https?:\/\//.test(text)) return null;
  if (/^(?:bg|text|border|ring|fill|stroke|from|to|via)-[a-z]+-\d{2,3}$/.test(text)) return null;
  if (
    /^(?:transcription|summary|network|server|storage|validation|generic|MiB|MB|GiB)$/.test(
      text,
    )
  ) {
    return null;
  }
  if (
    ts.isCallExpression(node.parent) &&
    node.parent.expression.getText() === "t"
  ) {
    return null;
  }
  const enclosingAttribute = enclosingJsxAttribute(node);
  if (
    enclosingAttribute &&
    !["title", "placeholder", "alt", "aria-label"].includes(
      enclosingAttribute.name.text,
    )
  ) {
    return null;
  }
  if (ts.isBinaryExpression(node.parent)) return null;
  if (ts.isJsxText(node)) return "jsx-text";
  if (
    ts.isJsxAttribute(node.parent) &&
    ["title", "placeholder", "alt", "aria-label"].includes(node.parent.name.text)
  ) {
    return "jsx-attribute:" + node.parent.name.text;
  }
  if (
    ts.isPropertyAssignment(node.parent) &&
    ["title", "description", "label", "placeholder", "message"].includes(
      propertyName(node.parent),
    )
  ) {
    return "object-property:" + propertyName(node.parent);
  }
  if (ts.isReturnStatement(node.parent)) return "returned-ui-string";
  if (isInsideJsxExpression(node)) return "jsx-expression";
  if (ts.isCallExpression(node.parent)) {
    const expression = node.parent.expression.getText();
    if (
      expression === "alert" ||
      expression === "confirm" ||
      /^toast\.(error|info|success|warning)$/.test(expression)
    ) {
      return "call:" + expression;
    }
  }
  return null;
}

const resourceFindings = [];
const resourceStats = [];
for (const namespace of batch.namespaces) {
  const enPath = path.join(root, "frontend/src/i18n/locales/en/" + namespace + ".json");
  const zhPath = path.join(root, "frontend/src/i18n/locales/zh-CN/" + namespace + ".json");
  const en = flatten(JSON.parse(await fs.readFile(enPath, "utf8")));
  const zh = flatten(JSON.parse(await fs.readFile(zhPath, "utf8")));
  const keys = [...new Set([...en.keys(), ...zh.keys()])].sort();

  for (const key of keys) {
    const id = namespace + ":" + key;
    const enValue = en.get(key);
    const zhValue = zh.get(key);
    if (!en.has(key) || !zh.has(key)) {
      resourceFindings.push({
        id,
        kind: !en.has(key) ? "missing-en-key" : "missing-zh-CN-key",
      });
      continue;
    }
    if (
      typeof enValue !== "string" ||
      typeof zhValue !== "string" ||
      !enValue.trim() ||
      !zhValue.trim()
    ) {
      resourceFindings.push({ id, kind: "invalid-or-empty-value" });
      continue;
    }
    if (/\b(?:TODO|TBD|TRANSLATE_ME)\b/i.test(zhValue)) {
      resourceFindings.push({ id, kind: "translation-marker" });
    }
    if (JSON.stringify(placeholders(enValue)) !== JSON.stringify(placeholders(zhValue))) {
      resourceFindings.push({
        id,
        kind: "placeholder-mismatch",
        en: placeholders(enValue),
        zhCN: placeholders(zhValue),
      });
    }
    if (
      enValue === zhValue &&
      /[A-Za-z]{2,}(?:\s+[A-Za-z]{2,})+/.test(enValue) &&
      !allowlist.resourceKeys.includes(id)
    ) {
      resourceFindings.push({
        id,
        kind: "unapproved-identical-english",
        value: enValue,
      });
    }
  }
  resourceStats.push({ namespace, enKeys: en.size, zhCNKeys: zh.size });
}

const sourceFindings = [];
for (const relativePath of batch.sources) {
  const sourceText = await fs.readFile(path.join(root, relativePath), "utf8");
  const sourceFile = ts.createSourceFile(
    relativePath,
    sourceText,
    ts.ScriptTarget.Latest,
    true,
    relativePath.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
  );
  function visit(node) {
    const text = literalText(node);
    const kind = text ? findingKind(node, text) : null;
    if (kind) {
      const position = sourcePosition(sourceFile, node);
      const id = relativePath + ":" + position.line + ":" + kind;
      if (!allowlist.sourceFindings.includes(id)) {
        sourceFindings.push({ id, file: relativePath, ...position, kind, text });
      }
    }
    ts.forEachChild(node, visit);
  }
  visit(sourceFile);
}

const report = {
  phase: "15.5-stage-2-react-frontend-migration",
  batch: batchId,
  batchName: batch.name,
  generatedAt: new Date().toISOString(),
  namespaces: batch.namespaces,
  sources: batch.sources,
  resourceStats,
  findings: { resources: resourceFindings, sources: sourceFindings },
  summary: {
    resourceFailures: resourceFindings.length,
    sourceFailures: sourceFindings.length,
    totalFailures: resourceFindings.length + sourceFindings.length,
    passed: resourceFindings.length === 0 && sourceFindings.length === 0,
  },
};

await fs.mkdir(path.dirname(reportPath), { recursive: true });
await fs.writeFile(reportPath, JSON.stringify(report, null, 2) + "\n");
process.stdout.write(JSON.stringify(report.summary) + "\n");
if (!report.summary.passed) process.exitCode = 1;
