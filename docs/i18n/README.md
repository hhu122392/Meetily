# Meetily 多语言工程

本目录是 Meetily 多语言改造的版本化工作区。阶段 0 与阶段 1 均已严格验收通过；阶段 2 已进入执行，2A、2B、2C 的 Windows x64 对应范围均已通过并记录为 CONDITIONAL PASS。2C 最终达到 `meetings` 79/79、`summary` 176/176、`templates` 275/275，i18n 24/24、会议模板 32/32、运行时 15/15、Next.js 13/13 和隔离 Tauri Release 验收；标题编辑缺口已恢复并通过物理编辑/保存测试。2D–2F、真实数据库/模型/音频矩阵与非 Windows 平台仍待执行，阶段 2 尚未整体放行。

## 当前阶段状态

| 阶段 | 状态 | 结果 |
|---|---|---|
| 15.3 阶段 0：冻结英文基线 | **PASS** | 静态 39/39、运行时 11/11；P0/P1/P2=0，详见严格验收报告 |
| 15.4 阶段 1：i18n 基础设施 | **PASS** | i18n 12/12；production 切换 10/10；冷启动 4/4；Tauri dev 4/4；最终 release 回归 4/4；P0/P1/P2=0 |
| 15.5 阶段 2：React 前端迁移 | **IN PROGRESS** | 2A、2B、2C Windows 对应范围 PASS、批次均为 CONDITIONAL PASS；2D–2F 待严格验收，阶段 2 尚未放行 |
| 15.6 阶段 3：Tauri 原生层 | 未开始 | 阶段 1 前置门禁已满足 |
| 15.7 阶段 4：模板和 AI 内容 | 未开始 | 阶段 1 前置门禁已满足 |
| 15.8 阶段 5：QA 与发布 | 未开始 | 等待阶段 0–4 |

## 核心文件

| 文件或目录 | 用途 |
|---|---|
| `i18n-plan.zh-CN.md` | 多语言架构、开发计划和阶段验收审计标准 |
| `phase-5a4/phase-5a4b-production-signing-audit.md` | 阶段 5A-4B 生产签名链、时间戳、更新签名和回滚集成严格审计 |
| `phase-5a4/phase-5a4b-production-signing-runbook.zh-CN.md` | Windows 生产证书批准、CI、双签名验证、轮换与故障处置手册 |
| `phase-5a4/windows-signing-policy.v1.json` | Windows 发布者主体、角色化证书指纹、EKU、时间戳和 Tauri 更新签名策略 |
| `phase-5a4/phase-5a4c1-certificate-admission-audit.md` | 阶段 5A-4C-1 生产证书准入预审、失败关闭和真实环境严格审计 |
| `phase-5a4/phase-5a4c1-certificate-admission-runbook.zh-CN.md` | Windows 生产证书候选证据、双审批、持钥证明、轮换和吊销操作手册 |
| `phase-5a4/windows-certificate-admission.v1.json` | 当前生产证书的受控准入状态、公开证据、双审批和准入要求 |
| `baseline/locales/en/*.json` | 已冻结的正式英文资源，共 12 个 Namespace、710 个键 |
| `baseline/source-map.json` | 源码出现位置到正式语义键的完整映射 |
| `baseline/disposition.json` | 1354 条去重目录记录的最终处置 |
| `baseline/placeholder-migration.json` | 24 条显式占位符/拆键迁移计划 |
| `baseline/placeholders.json` | 正式英文资源的全部占位符清单 |
| `baseline/native-visibility-matrix.json` | 447 条活跃 Rust/Tauri 候选及 45 条旧代码记录的可见性和后续动作 |
| `baseline/template-content-inventory.json` | 104 条模板和 AI Prompt 内容清单 |
| `baseline/do-not-translate.json` | 品牌、模型、协议、文件格式和代码标识符不可翻译规则 |
| `baseline/glossary.en-zh-CN.json` | 已批准的英文/简体中文核心术语表 |
| `baseline/phase0.manifest.json` | 冻结版本、Commit、数量和产物清单 |
| `audit/phase-0-baseline/phase0-audit-report.md` | 阶段 0 人类可读审计报告 |
| `audit/phase-0-baseline/phase0-audit-report.json` | 阶段 0 机器可读审计报告 |
| `audit/phase-1-runtime/phase1-audit-report.md` | 阶段 1 严格验收、真实运行时矩阵、构建证据和放行决定 |
| `audit/phase-1-runtime/phase1-audit-report.json` | 阶段 1 机器可读验收状态 |
| `audit/phase-1-runtime/cdp-switch/` | Production 语言切换、状态隔离、离线和事件路径证据 |
| `audit/phase-1-runtime/cdp-cold/` | Production 进程级冷启动与首屏门禁证据 |
| `audit/phase-2-react/2B/phase2-2B-audit-report.md` | 2B 侧栏、首页、录音与实时转写严格验收报告 |
| `audit/phase-2-react/2C/phase2-2C-audit-report.md` | 2C 会议详情、标题、摘要、会议模板与语言隔离严格验收报告 |
| `audit/phase-2-react/2C/runtime-final/` | 2C 最终 SHA-256 对应的 15 项 CDP 运行时证据 |
| `audit/phase-1-runtime/cdp-dev/` | Tauri dev、Turbopack、IPC 与 dev CSP 证据 |
| `audit/phase-1-runtime/cdp-final-release/` | 最终 SHA-256 对应二进制的冷启动回归证据 |
| `scripts/runtime-phase1-i18n-audit.mjs` | 阶段 1 CDP 真实键鼠、状态、离线、冷启动和 dev 审计脚本 |
| `scripts/install-windows-build-tools.ps1` | Windows C++ Build Tools 发现、安装和验证辅助脚本 |
| `scripts/extract-i18n-candidates.mjs` | TypeScript、Rust 和模板文本候选扫描器 |
| `scripts/freeze-phase0-baseline.mjs` | 从扫描目录生成正式阶段 0 基线 |
| `scripts/audit-phase0.mjs` | 阶段 0 强制门禁脚本 |

## 阶段 0 冻结结果

| 指标 | 结果 |
|---|---:|
| 原始文本出现位置 | 1549 |
| 去重目录记录 | 1354 |
| 已确认前端翻译项 | 696 |
| 前端人工复核项 | 62，全部完成处置 |
| 正式源码映射 | 745 |
| 正式英文翻译键 | 710 |
| 英文 Namespace | 12 |
| Rust/Tauri 活跃候选 | 447，全部完成分类 |
| 旧 Rust 源码项 | 45，全部隔离 |
| 模板/AI 内容项 | 104，全部进入独立内容清单 |
| 原始确认复杂表达式 | 20，全部完成简单化 |
| 复核中新增的显式迁移 | 4 |
| 正式资源复杂占位符 | 0 |
| 哈希碰撞键 | 0 |
| 阶段 0 自动静态审计 | 39/39 `PASS` |
| 前端生产构建 | `PASS`，退出码 0 |

24 条显式迁移由 20 条原始确认复杂表达式和复核时发现的 4 条补充规范化组成。补充项包括两条间接状态表达式，以及两处需要把硬编码模型名改成变量的文案。

## 正式英文 Namespace

```text
analytics
common
import
meetings
models
navigation
onboarding
recording
settings
summary
transcription
updates
```

正式键只使用业务域和语义类别，不暴露 `components`、`hooks`、`contexts`、组件文件名或哈希碰撞后缀。

## 重新生成和审计

在仓库根目录执行：

```powershell
$repoRoot = (Resolve-Path -LiteralPath '.').Path

node docs\i18n\scripts\extract-i18n-candidates.mjs `
  $repoRoot `
  (Join-Path $repoRoot 'docs\i18n\baseline\source-text-candidates.json')

node docs\i18n\scripts\freeze-phase0-baseline.mjs $repoRoot

Set-Location frontend
pnpm run build
Set-Location ..

node docs\i18n\scripts\audit-phase0.mjs $repoRoot
```

审计脚本会在任一强制检查失败时返回非零退出码。

## 阶段 0 的边界

阶段 0 只冻结资源、映射、术语、处置和审计证据，不修改 React/Tauri 运行时，不安装 `i18next`，也不提供中文 UI。正式运行时接入和 `zh-CN` 资源创建属于阶段 1 与阶段 2。

当前项目的 `frontend/package.json` 没有统一 `test` 脚本，因此阶段 0 将该项记录为 `N/A`；生产构建已经完成 TypeScript 有效性检查并成功生成全部静态页面。
