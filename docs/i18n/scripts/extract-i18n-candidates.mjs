import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';

const repoRoot = path.resolve(process.argv[2] || '.');
const outputPath = path.resolve(
  process.argv[3] || path.join(repoRoot, 'target', 'release', 'docs', 'source-text-candidates.json'),
);
const frontendRoot = path.join(repoRoot, 'frontend');
const requireFromFrontend = createRequire(path.join(frontendRoot, 'package.json'));
const ts = requireFromFrontend('typescript');

const candidates = [];
const seen = new Set();

function walk(directory, predicate) {
  const results = [];
  if (!fs.existsSync(directory)) return results;
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const fullPath = path.join(directory, entry.name);
    if (entry.isDirectory()) results.push(...walk(fullPath, predicate));
    else if (predicate(fullPath)) results.push(fullPath);
  }
  return results;
}

function normalizeText(value) {
  return value
    .replace(/\r?\n/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

function containsNaturalLanguage(value) {
  const text = normalizeText(value);
  if (!/[A-Za-z]/.test(text)) return false;
  if (/^(https?:|mailto:|[.#/@]|[a-z]+:\/\/)/i.test(text)) return false;
  if (/^[A-Za-z0-9_.:/-]+$/.test(text) && !text.includes(' ')) return false;
  return true;
}

function addCandidate({ layer, kind, text, source, line, context, review = 'translate' }) {
  const normalized = normalizeText(text);
  if (!containsNaturalLanguage(normalized)) return;
  const relativeSource = path.relative(repoRoot, source).replaceAll('\\', '/');
  const signature = `${layer}|${kind}|${relativeSource}|${line}|${normalized}`;
  if (seen.has(signature)) return;
  seen.add(signature);
  candidates.push({ layer, kind, text: normalized, source: relativeSource, line, context, review });
}

function lineOf(sourceFile, node) {
  return sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile)).line + 1;
}

function literalText(node) {
  if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) return node.text;
  if (ts.isTemplateExpression(node)) {
    let value = node.head.text;
    for (const span of node.templateSpans) {
      value += `{${span.expression.getText()}}${span.literal.text}`;
    }
    return value;
  }
  return null;
}

function expressionTexts(node) {
  const direct = literalText(node);
  if (direct !== null) return [direct];
  if (ts.isParenthesizedExpression(node) || ts.isAsExpression(node) || ts.isTypeAssertionExpression(node)) {
    return expressionTexts(node.expression);
  }
  if (ts.isConditionalExpression(node)) {
    return [...expressionTexts(node.whenTrue), ...expressionTexts(node.whenFalse)];
  }
  if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.PlusToken) {
    const left = expressionTexts(node.left);
    const right = expressionTexts(node.right);
    if (left.length === 1 && right.length === 1) return [`${left[0]}${right[0]}`];
  }
  return [];
}

const uiAttributeNames = new Set([
  'aria-label', 'title', 'placeholder', 'alt', 'label', 'description', 'message',
  'tooltip', 'emptyText', 'loadingText', 'confirmText', 'cancelText', 'helperText',
]);
const uiPropertyNames = new Set([
  'title', 'description', 'label', 'message', 'userMessage', 'tooltip', 'placeholder',
  'emptyText', 'loadingText', 'confirmText', 'cancelText', 'detail', 'body', 'name',
]);

function scanTypeScript(filePath) {
  const sourceText = fs.readFileSync(filePath, 'utf8');
  const scriptKind = filePath.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS;
  const sourceFile = ts.createSourceFile(filePath, sourceText, ts.ScriptTarget.Latest, true, scriptKind);

  function visit(node) {
    if (ts.isJsxText(node)) {
      addCandidate({
        layer: 'frontend', kind: 'jsx_text', text: node.getText(sourceFile), source: filePath,
        line: lineOf(sourceFile, node), context: 'Rendered JSX text',
      });
    }

    // Only collect expressions rendered as JSX children. Expressions used as
    // attributes (especially className template strings) are implementation
    // details and must never enter a locale bundle.
    if (ts.isJsxExpression(node) && node.expression && !ts.isJsxAttribute(node.parent)) {
      for (const text of expressionTexts(node.expression)) {
        const looksLikeDisplayedCodeSample = /^\{\s*["'][A-Za-z0-9_]+["']\s*:/.test(normalizeText(text));
        addCandidate({
          layer: 'frontend', kind: 'jsx_expression', text, source: filePath,
          line: lineOf(sourceFile, node), context: 'Rendered JSX expression',
          review: looksLikeDisplayedCodeSample ? 'review' : 'translate',
        });
      }
    }

    if (ts.isJsxAttribute(node)) {
      const name = node.name.getText(sourceFile);
      if (uiAttributeNames.has(name) && node.initializer) {
        let texts = [];
        if (ts.isStringLiteral(node.initializer)) texts = [node.initializer.text];
        else if (ts.isJsxExpression(node.initializer) && node.initializer.expression) {
          texts = expressionTexts(node.initializer.expression);
        }
        for (const text of texts) {
          addCandidate({
            layer: 'frontend', kind: `jsx_attribute:${name}`, text, source: filePath,
            line: lineOf(sourceFile, node), context: `User-facing ${name} attribute`,
          });
        }
      }
    }

    if (ts.isPropertyAssignment(node)) {
      const propertyName = node.name.getText(sourceFile).replace(/^['"]|['"]$/g, '');
      if (uiPropertyNames.has(propertyName)) {
        const text = literalText(node.initializer);
        if (text !== null) {
          addCandidate({
            layer: 'frontend', kind: `object_property:${propertyName}`, text, source: filePath,
            line: lineOf(sourceFile, node), context: `Potentially user-facing ${propertyName} property`,
            review: propertyName === 'name' ? 'review' : 'translate',
          });
        }
      }
    }

    if (ts.isCallExpression(node)) {
      const callee = node.expression.getText(sourceFile);
      const isUserMessageCall = /^(toast\.(success|error|warning|info|message)|alert|confirm|window\.alert|window\.confirm|prompt|window\.prompt)$/.test(callee);
      if (isUserMessageCall) {
        for (const argument of node.arguments) {
          for (const text of expressionTexts(argument)) {
            addCandidate({
              layer: 'frontend', kind: `call:${callee}`, text, source: filePath,
              line: lineOf(sourceFile, argument), context: `Message passed to ${callee}`,
            });
          }
        }
      }

      const maySetVisibleText = /(^|\.)(set[A-Za-z]*(Error|Message|Title|Description|Label|Status)|showError|showMessage|notify)$/.test(callee);
      if (maySetVisibleText) {
        for (const argument of node.arguments) {
          for (const text of expressionTexts(argument)) {
            addCandidate({
              layer: 'frontend', kind: `indirect_call:${callee}`, text, source: filePath,
              line: lineOf(sourceFile, argument), context: `Text passed to ${callee}; verify that it is rendered`,
              review: 'review',
            });
          }
        }
      }
    }

    if (ts.isNewExpression(node) && node.expression.getText(sourceFile) === 'Error') {
      for (const argument of node.arguments || []) {
        for (const text of expressionTexts(argument)) {
          addCandidate({
            layer: 'frontend', kind: 'new_error', text, source: filePath,
            line: lineOf(sourceFile, argument), context: 'Thrown Error text; verify whether it reaches the user interface',
            review: 'review',
          });
        }
      }
    }

    ts.forEachChild(node, visit);
  }

  visit(sourceFile);
}

const tsFiles = walk(path.join(frontendRoot, 'src'), (filePath) => /\.(tsx?|jsx?)$/.test(filePath));
for (const filePath of tsFiles) scanTypeScript(filePath);

function unescapeRust(value) {
  return value.replace(/\\n/g, ' ').replace(/\\"/g, '"').replace(/\\\\/g, '\\');
}

const rustFiles = walk(path.join(frontendRoot, 'src-tauri', 'src'), (filePath) => filePath.endsWith('.rs'));
for (const filePath of rustFiles) {
  const lines = fs.readFileSync(filePath, 'utf8').split(/\r?\n/);
  lines.forEach((line, index) => {
    const isLogOnly = /\b(trace|debug|info|warn|error|log_(info|warn|error|debug))!\s*\(/.test(line);
    const hasUserContext = /userMessage|\.title\s*\(|\.body\s*\(|MenuItem|CheckMenuItem|Err\s*\(|"message"\s*:|format!\s*\(/.test(line);
    if (!hasUserContext || isLogOnly) return;
    const stringPattern = /"((?:\\.|[^"\\])*)"/g;
    for (const match of line.matchAll(stringPattern)) {
      addCandidate({
        layer: 'tauri', kind: 'rust_user_candidate', text: unescapeRust(match[1]),
        source: filePath, line: index + 1,
        context: 'Native menu, notification, command result, or error candidate', review: 'review',
      });
    }
  });
}

const templateRoots = [
  path.join(repoRoot, 'templates'),
  path.join(frontendRoot, 'src-tauri', 'templates'),
  path.join(frontendRoot, 'public', 'templates'),
];

function visitJson(value, filePath, jsonPath = '$') {
  if (typeof value === 'string') {
    addCandidate({
      layer: 'template', kind: 'json_string', text: value, source: filePath, line: null,
      context: `Template content at ${jsonPath}`, review: 'separate_content_localization',
    });
  } else if (Array.isArray(value)) {
    value.forEach((item, index) => visitJson(item, filePath, `${jsonPath}[${index}]`));
  } else if (value && typeof value === 'object') {
    for (const [key, item] of Object.entries(value)) visitJson(item, filePath, `${jsonPath}.${key}`);
  }
}

for (const root of templateRoots) {
  for (const filePath of walk(root, (candidate) => candidate.endsWith('.json'))) {
    try {
      visitJson(JSON.parse(fs.readFileSync(filePath, 'utf8')), filePath);
    } catch {
      // Invalid or commented JSON is left for manual review.
    }
  }
}

candidates.sort((a, b) =>
  a.layer.localeCompare(b.layer) || a.source.localeCompare(b.source) || (a.line ?? 0) - (b.line ?? 0),
);

const byLayer = Object.fromEntries(
  [...new Set(candidates.map((item) => item.layer))].map((layer) => [
    layer,
    candidates.filter((item) => item.layer === layer).length,
  ]),
);
const byReview = Object.fromEntries(
  [...new Set(candidates.map((item) => item.review))].map((review) => [
    review,
    candidates.filter((item) => item.review === review).length,
  ]),
);

const report = {
  schemaVersion: 1,
  generatedAt: new Date().toISOString(),
  repository: repoRoot.replaceAll('\\', '/'),
  scope: {
    typescriptFiles: tsFiles.length,
    rustFiles: rustFiles.length,
    note: 'Machine-generated candidates. Translate/review flags require human validation; developer logs and protocol identifiers are intentionally excluded where recognizable.',
  },
  summary: { total: candidates.length, byLayer, byReview },
  candidates,
};

fs.writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
console.log(JSON.stringify({ outputPath, ...report.summary }, null, 2));
