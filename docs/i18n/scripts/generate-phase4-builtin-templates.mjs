#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

const root = path.resolve(process.argv[2] || ".");
const sourceDirectory = path.join(root, "frontend/src-tauri/templates");
const generatedAt = "2026-08-23T00:00:00+08:00";

const zh = new Map(Object.entries({
  "Daily Standup": "每日站会",
  "Time-boxed daily updates for engineering/product teams.": "适用于工程和产品团队的限时每日进展同步。",
  "Date": "日期",
  "Attendees": "参会人",
  "List of participants present": "列出实际参会人员",
  "Yesterday": "昨日完成",
  "What I completed yesterday (short bullets)": "用简短要点列出昨日完成的工作",
  "| **Owner** | **Completed Work** |\n| --- | --- |": "| **负责人** | **已完成工作** |\n| --- | --- |",
  "Today": "今日计划",
  "Planned work for today (short bullets)": "用简短要点列出今天计划完成的工作",
  "| **Owner** | **Planned Work** |\n| --- | --- |": "| **负责人** | **计划工作** |\n| --- | --- |",
  "Blockers": "阻塞项",
  "Any impediments and owner if known": "列出所有阻塞项；如已知，请注明负责人",
  "| **Owner** | **Blocker** | Impact |\n| --- | --- | --- |": "| **负责人** | **阻塞项** | **影响** |\n| --- | --- | --- |",
  "Notes": "备注",
  "Optional quick notes or announcements": "补充简短备注或通知（可选）",

  "Project Sync / Status Update": "项目同步／状态更新",
  "Weekly or bi-weekly project status meeting focusing on milestones and risks.": "用于每周或双周项目状态会议，重点关注里程碑和风险。",
  "Meeting Date & Time": "会议日期与时间",
  "Date, start/end time, facilitator name": "记录日期、开始／结束时间和主持人姓名",
  "List attendees and their roles": "列出参会人及其角色",
  "Milestones & Status": "里程碑与状态",
  "Current milestones with status and estimated completion date": "列出当前里程碑、状态及预计完成日期",
  "| **Milestone** | **Status** | **ETA** |\n| --- | --- | --- |": "| **里程碑** | **状态** | **预计完成时间** |\n| --- | --- | --- |",
  "Progress Summary": "进展摘要",
  "Short paragraph summarizing progress since last sync meeting": "用一小段文字概括自上次同步会议以来的进展",
  "Top Risks & Mitigations": "主要风险与缓解措施",
  "List top risks with impact level, mitigation plan, and owner": "列出主要风险、影响程度、缓解计划和负责人",
  "| **Risk** | **Impact** | **Mitigation** | **Owner** |\n| --- | --- | --- | --- |": "| **风险** | **影响** | **缓解措施** | **负责人** |\n| --- | --- | --- | --- |",
  "Key Decisions": "关键决策",
  "Decisions made in this meeting with rationale and timestamp": "记录本次会议作出的决策、理由和时间戳",
  "| **Decision** | **Rationale** | **Timestamp** |\n| --- | --- | --- |": "| **决策** | **理由** | **时间戳** |\n| --- | --- | --- |",
  "Action Items": "行动项",
  "Tasks with owners, due dates, priority, and status": "列出任务、负责人、截止日期、优先级和状态",
  "| **Owner** | **Task** | **Due Date** | **Priority** | **Status** |\n| --- | --- | --- | --- | --- |": "| **负责人** | **任务** | **截止日期** | **优先级** | **状态** |\n| --- | --- | --- | --- | --- |",
  "Related Documents": "相关文档",
  "Links to relevant documents, tickets, or designs discussed": "列出讨论中涉及的相关文档、工单或设计链接",
  "| **Document Title** | **URL** | **Type** |\n| --- | --- | --- |": "| **文档标题** | **URL** | **类型** |\n| --- | --- | --- |",

  "Psychiatric Session Note (SOAP + AI Hybrid)": "精神科会谈记录（SOAP + AI 混合）",
  "AI-assisted psychiatric progress note template based on SOAP, with clinical metadata and AI summary.": "基于 SOAP 的 AI 辅助精神科进展记录模板，包含临床元数据和 AI 摘要。",
  "Session Metadata": "会谈元数据",
  "Patient initials/ID, session date, provider, session type, duration (HIPAA-sensitive)": "记录患者姓名首字母／ID、会谈日期、服务提供者、会谈类型和时长（HIPAA 敏感信息）",
  "AI Session Summary": "AI 会谈摘要",
  "One-line takeaway and one-paragraph executive summary (AI-generated). Include confidence score.": "生成一句话结论和一段式执行摘要（由 AI 生成），并包含置信度。",
  "Subjective (S)": "主观资料（S）",
  "Patient's self-reported mood, symptoms, sleep, appetite, stressors": "记录患者自述的情绪、症状、睡眠、食欲和压力源",
  "Objective (O)": "客观资料（O）",
  "Observable findings: affect, speech, behavior, orientation, vitals if relevant": "记录可观察结果：情感表现、言语、行为、定向力，以及相关生命体征",
  "Assessment (A)": "评估（A）",
  "Diagnostic impression, risk assessment (SI/HI), progress vs treatment goals": "记录诊断印象、风险评估（SI/HI）及相对于治疗目标的进展",
  "Plan (P)": "计划（P）",
  "Interventions, medication changes, referrals, therapy tasks and follow-up": "记录干预措施、药物调整、转诊、治疗任务和随访安排",
  "| **Intervention** | **Owner** | **Frequency** | **Follow-up Date** |\n| --- | --- | --- | --- |": "| **干预措施** | **负责人** | **频率** | **随访日期** |\n| --- | --- | --- | --- |",
  "Medications": "药物",
  "Current medications, doses, changes, and rationale": "记录当前药物、剂量、调整情况及理由",
  "| **Medication** | **Dose** | **Route** | **Start/Change Date** |\n| --- | --- | --- | --- |": "| **药物** | **剂量** | **给药途径** | **开始／调整日期** |\n| --- | --- | --- | --- |",
  "Diagnoses (DSM/ICD)": "诊断（DSM/ICD）",
  "List diagnostic codes and working diagnosis": "列出诊断编码和工作诊断",
  "| **Code** | **System** | **Diagnosis** |\n| --- | --- | --- |": "| **编码** | **体系** | **诊断** |\n| --- | --- | --- |",
  "Safety & Risk Management": "安全与风险管理",
  "Safety plan, emergency contacts, hospitalization considerations": "记录安全计划、紧急联系人及住院考量",
  "Next Appointment": "下次预约",
  "Date, time and modality of next session": "记录下次会谈的日期、时间和形式",
  "Audit Trail": "审计记录",
  "Human review flag, reviewer name and timestamp, AI version": "记录人工复核标记、复核人姓名、时间戳和 AI 版本",

  "Retrospective (Agile)": "敏捷回顾会",
  "Sprint retrospective template for continuous improvement.": "用于持续改进的迭代回顾会模板。",
  "Sprint": "迭代",
  "Sprint name/number and date range": "记录迭代名称／编号和日期范围",
  "Attendance": "出席情况",
  "List of participants": "列出参会人员",
  "Start Doing": "开始做",
  "Actions or experiments to start next sprint": "列出下个迭代开始执行的行动或实验",
  "| **Idea** | **Proposer** |\n| --- | --- |": "| **想法** | **提出人** |\n| --- | --- |",
  "Stop Doing": "停止做",
  "Practices to stop": "列出需要停止的做法",
  "| **Practice** | **Reason** |\n| --- | --- |": "| **做法** | **原因** |\n| --- | --- |",
  "Continue Doing": "继续做",
  "Practices to continue": "列出需要继续保持的做法",
  "| **Practice** | **Notes** |\n| --- | --- |": "| **做法** | **备注** |\n| --- | --- |",
  "Concrete experiments with owners and success metrics": "列出具体实验、负责人和成功指标",
  "| **Owner** | **Task** | **Due Date** | **Success Metric** |\n| --- | --- | --- | --- |": "| **负责人** | **任务** | **截止日期** | **成功指标** |\n| --- | --- | --- | --- |",
  "Notes & Votes": "备注与投票",
  "Summary and top-voted items": "汇总讨论内容和得票最高的事项",

  "Client / Sales Meeting": "客户／销售会议",
  "Capture client goals, deliverables, and next steps.": "记录客户目标、交付物和后续步骤。",
  "Meeting Metadata": "会议元数据",
  "Date, time, location/modality, account manager": "记录日期、时间、地点／会议形式和客户经理",
  "Client and vendor attendees with roles": "列出客户方与供应方参会人及其角色",
  "Client Goals & Success Criteria": "客户目标与成功标准",
  "What the client wants to achieve and how success will be measured": "记录客户希望达成的目标及成功衡量方式",
  "Agreed Deliverables": "约定交付物",
  "Deliverables, owners, and due dates": "列出交付物、负责人和截止日期",
  "| **Deliverable** | **Owner** | **Due Date** |\n| --- | --- | --- |": "| **交付物** | **负责人** | **截止日期** |\n| --- | --- | --- |",
  "Commercial Terms Discussed": "已讨论的商务条款",
  "Pricing, SLAs, payment terms or contract items discussed": "记录已讨论的定价、SLA、付款条款或合同事项",
  "Risks & Concerns": "风险与顾虑",
  "Client concerns, blockers, or escalation items": "记录客户顾虑、阻塞项或需升级处理的事项",
  "| **Concern** | **Impact** | **Owner** |\n| --- | --- | --- |": "| **顾虑** | **影响** | **负责人** |\n| --- | --- | --- |",
  "Next Steps": "后续步骤",
  "Actions, owners, and due dates": "列出行动、负责人和截止日期",
  "| **Owner** | **Action** | **Due Date** |\n| --- | --- | --- |": "| **负责人** | **行动** | **截止日期** |\n| --- | --- | --- |",

  "Standard Meeting Notes": "标准会议纪要",
  "A standard template for general meetings, focusing on key outcomes and actions.": "适用于一般会议的标准模板，重点记录关键成果和行动。",
  "Summary": "摘要",
  "Provide a brief, one-paragraph executive summary of the entire meeting.": "用一段简短文字概括整场会议的核心内容。",
  "List the most important decisions made during the meeting.": "列出会议中作出的最重要决策。",
  "List all assigned tasks with their owners and due date. Always add reference transcript segment and timestamp in the table.": "列出所有已分配任务、负责人和截止日期，并始终在表格中附上对应的转录片段和时间戳。",
  "| **Owner** | Task | Due | Reference Transcript Segment | Segment Time stamp |\n| --- | --- | --- | --- | --- |": "| **负责人** | **任务** | **截止时间** | **对应转录片段** | **片段时间戳** |\n| --- | --- | --- | --- | --- |",
  "Discussion Highlights": "讨论要点",
  "Summarize the main topics of discussion, key arguments, and important insights.": "概括主要讨论主题、关键论点和重要见解。",

  "YYYY-MM-DD": "YYYY-MM-DD"
}));

function slug(value, fallback) {
  const normalized = value
    .normalize("NFKD")
    .replace(/[^A-Za-z0-9 _.-]/g, "")
    .toLowerCase()
    .replace(/[ .-]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .slice(0, 80);
  return normalized || fallback;
}

function translate(value, context) {
  const translated = zh.get(value);
  if (translated === undefined) {
    throw new Error(`Missing zh-CN translation for ${context}: ${JSON.stringify(value)}`);
  }
  return translated;
}

function localizeLegacyTemplate(id, legacy, locale) {
  const used = new Set();
  const sections = legacy.sections.map((section, index) => {
    let sectionId = slug(section.title, `section_${index + 1}`);
    const base = sectionId;
    let suffix = 2;
    while (used.has(sectionId)) sectionId = `${base}_${suffix++}`;
    used.add(sectionId);
    const localized = (value, field) =>
      locale === "en" ? value : translate(value, `${id}.sections[${index}].${field}`);
    return {
      id: sectionId,
      title: localized(section.title, "title"),
      instruction: localized(section.instruction, "instruction"),
      format: section.format,
      item_format: section.item_format ? localized(section.item_format, "item_format") : null,
      example_item_format: section.example_item_format
        ? localized(section.example_item_format, "example_item_format")
        : null,
      required: true,
      empty_behavior: "show_not_mentioned"
    };
  });

  return {
    schema_version: 2,
    id,
    name: locale === "en" ? legacy.name : translate(legacy.name, `${id}.name`),
    description:
      locale === "en"
        ? legacy.description
        : translate(legacy.description, `${id}.description`),
    version: 1,
    locale,
    tags: [],
    source: {
      type: "builtin",
      original_file_name: null,
      original_file_sha256: null,
      imported_at: null,
      copied_from_template_id: null
    },
    created_at: generatedAt,
    updated_at: generatedAt,
    sections,
    extensions: {
      "meetily.content_revision": 1
    }
  };
}

const inputFiles = (await fs.readdir(sourceDirectory))
  .filter((file) => file.endsWith(".json"))
  .sort();
const generated = [];
for (const file of inputFiles) {
  const id = file.slice(0, -5);
  const legacy = JSON.parse(await fs.readFile(path.join(sourceDirectory, file), "utf8"));
  for (const locale of ["en", "zh-CN"]) {
    const targetDirectory = path.join(sourceDirectory, locale);
    await fs.mkdir(targetDirectory, { recursive: true });
    const target = path.join(targetDirectory, file);
    const template = localizeLegacyTemplate(id, legacy, locale);
    await fs.writeFile(target, `${JSON.stringify(template, null, 2)}\n`);
    generated.push(path.relative(root, target).replaceAll("\\", "/"));
  }
}

process.stdout.write(
  `${JSON.stringify({ generated: generated.length, files: generated, translations: zh.size })}\n`,
);
