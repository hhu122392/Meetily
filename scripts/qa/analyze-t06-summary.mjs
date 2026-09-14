import fs from "node:fs";

const tracePath = process.argv[2];
if (!tracePath) throw new Error("Usage: node analyze-t06-summary.mjs <monitor-result.json>");
const trace = JSON.parse(fs.readFileSync(tracePath, "utf8"));
const summary = trace.finalText ?? "";
const newest = trace.newest ?? trace.historyAfter?.[0] ?? null;

const checks = {
  nonEmpty: summary.trim().length > 200,
  sections: {
    summary: summary.includes("摘要"),
    decisions: summary.includes("关键决策") || summary.includes("会议结论"),
    actionItems: summary.includes("行动项") || summary.includes("行动事项"),
    discussionOrRisks: summary.includes("讨论要点") || summary.includes("风险"),
  },
  entities: {
    李明: summary.includes("李明"),
    王芳: summary.includes("王芳"),
    赵强: summary.includes("赵强"),
    M100: summary.includes("M100"),
    八月二十八日: /八月二十八日|8\s*月\s*28\s*日/.test(summary),
  },
  responsibilities: {
    王芳提交真实测试报告: summary.includes("王芳") && summary.includes("提交真实测试报告"),
    赵强修复摘要生成失败和重复报错:
      summary.includes("赵强") && summary.includes("修复摘要生成失败和重复报错"),
  },
  risks: {
    录音延迟: summary.includes("录音延迟"),
    整段漏转: summary.includes("整段漏转"),
    繁体字输出: summary.includes("繁体字输出"),
    摘要生成失败: summary.includes("摘要生成失败"),
  },
};

const flatten = (value) => Object.values(value).flatMap((item) =>
  typeof item === "object" && item !== null ? flatten(item) : [item],
);
const failures = [];
for (const [group, values] of Object.entries(checks)) {
  if (typeof values === "boolean") {
    if (!values) failures.push(group);
    continue;
  }
  for (const [name, passed] of Object.entries(values)) {
    if (!passed) failures.push(`${group}.${name}`);
  }
}

console.log(JSON.stringify({
  analyzedAt: new Date().toISOString(),
  tracePath,
  generationId: newest?.generationId ?? null,
  model: newest ? `${newest.modelProvider}/${newest.modelName}` : null,
  template: newest ? `${newest.templateId}@${newest.templateVersion}` : null,
  summaryLanguage: newest?.summaryLanguage ?? null,
  summary,
  checks,
  allRequiredChecksPass: flatten(checks).every(Boolean),
  failures,
  verdict: failures.length === 0 ? "PASS" : "FAIL",
}, null, 2));
