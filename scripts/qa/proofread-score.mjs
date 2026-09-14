// Compare actual effects, allowing a longer context span only when its result is identical.
function effect(text, edit) {
  if (typeof edit.original !== 'string' || typeof edit.suggested !== 'string' || !edit.original) return null;
  const chars = Array.from(text);
  const start = edit.start_char ?? Array.from(text.slice(0, text.indexOf(edit.original))).length;
  const end = edit.end_char ?? start + Array.from(edit.original).length;
  if (text.indexOf(edit.original) < 0 || chars.slice(start, end).join('') !== edit.original) return null;
  if (edit.start_char == null && text.indexOf(edit.original, text.indexOf(edit.original) + 1) !== -1) return null;
  return chars.slice(0, start).join('') + edit.suggested + chars.slice(end).join('');
}

export function judge(parsed, testCase) {
  const expected = testCase.expect ?? [];
  const edits = parsed.candidates ?? [];
  const matches = (actual, wanted) => actual.segment_index === wanted.segment &&
    effect(testCase.segments[wanted.segment], actual) !== null &&
    effect(testCase.segments[wanted.segment], actual) === effect(testCase.segments[wanted.segment], wanted);
  const hits = expected.filter(wanted => edits.some(actual => matches(actual, wanted)));
  const misses = expected.filter(wanted => !hits.includes(wanted));
  const violations = edits.filter(actual => !expected.some(wanted => matches(actual, wanted)));
  const missingSegments = testCase.segments.map((_, i) => i).filter(i => !parsed.answered.includes(i));
  const incompleteAnnotation = testCase.annotation_complete !== true;
  const dropped = parsed.dropped ?? [];
  const conflicts = [];
  for (let i = 0; i < edits.length; i++) {
    const a = edits[i];
    const text = testCase.segments[a.segment_index];
    if (typeof text !== 'string' || effect(text, a) === null) continue;
    const startA = a.start_char ?? Array.from(text.slice(0, text.indexOf(a.original))).length;
    const endA = a.end_char ?? startA + Array.from(a.original).length;
    for (const b of edits.slice(i + 1)) {
      if (a.segment_index !== b.segment_index || effect(text, b) === null) continue;
      const startB = b.start_char ?? Array.from(text.slice(0, text.indexOf(b.original))).length;
      const endB = b.end_char ?? startB + Array.from(b.original).length;
      if (startA < endB && startB < endA) conflicts.push([a, b]);
    }
  }
  // A recovered malformed response remains visible and cannot certify model reliability.
  const pass = !incompleteAnnotation && !misses.length && !violations.length && !missingSegments.length && !dropped.length && !conflicts.length;
  return { pass, hits, misses, violations, missingSegments, dropped, conflicts, incompleteAnnotation };
}
