# Meetily 5A-1 发布源码纳入清单

日期：2026-08-23
基线：`main@0281737d87d26352fb0adc78c8c0975f691b23d1`
目的：把已经验证的中文化、会议模板和原生多语言实现整理成可审核、可拆分、可追溯的发布源码边界。

## 1. 纳入原则

只有满足以下条件的文件才可进入发布源码提交：

1. 属于产品运行时、生产构建、自动测试、审计工具或正式架构文档；
2. 已被 `source-inventory.json` 分配明确的 `category`、`disposition` 和 `commitGroup`；
3. 不属于生成的运行证据、隔离构建产物、恢复缓存、安装包或本机工具；
4. 强特征 Secret 扫描为 0；
5. 文件中如含混合变更、安全配置或非翻译行为修复，必须保留为独立审核项；
6. 正式产物不得从脏工作树直接发布，最终必须从批准提交重新构建。

逐文件路径、SHA-256、字节数、Git 状态和处置见 `docs/i18n/audit/phase-5a1-source/source-inventory.json`。该 JSON 是本清单的机器可读明细。

## 2. 生产源码及验证文件

| 提交组 | 范围 | 文件数 | 纳入结论 |
|---|---|---:|---|
| `01-repository-hygiene` | `.gitignore` 的隔离构建／恢复产物规则 | 1 | 纳入 |
| `02-build-and-dependencies` | Cargo/pnpm 清单与锁、构建脚本、Tauri/NSIS 配置 | 8 | 6 项直接纳入，2 项需逐项审核 |
| `03-supporting-regression-fixes` | Onboarding 状态加载、平台检测、编辑器导航保护 | 3 | 独立提交并审核 |
| `04-frontend-i18n` | React 多语言接入、13 Namespace、模板管理 UI | 133 | 纳入 |
| `05-native-and-template-engine` | Rust 原生 i18n、通知／托盘、模板 V2、内置模板 | 39 | 纳入 |
| `06-tests` | i18n、模板、导航和状态隔离测试 | 17 | 纳入 |

生产源码、构建与测试候选合计 201 个文件，其中 5 个文件保持显式审核状态，不允许被批量静默放行。

## 3. 审计和文档文件

| 提交组 | 范围 | 文件数 | 纳入结论 |
|---|---|---:|---|
| `07-audit-tooling` | 可重复审计脚本及三个隔离 Tauri 配置 | 46 | 独立提交；隔离配置不得替代生产配置 |
| `08-documentation` | 架构、基线、术语、阶段报告、发布方案及 5A-1 紧凑审计产物 | 51 | 49 项进入机器清单，另加 2 项自描述 JSON 输出；独立文档提交 |

审计工具与文档合计 97 个文件，计划纳入的发布源码总数为 298 个文件，不与产品运行时代码压成一个提交。`source-inventory.json` 和 `sensitive-content-audit.json` 为避免自递归不计入其自身的 958 项机器清单，但必须与另外 2 份 5A-1 紧凑审计文件一起提交。

## 4. 五个显式审核项

### 4.1 `frontend/package.json` — `REVIEW_MIXED_CHANGE`

应纳入：

- `i18next`、`react-i18next`；
- `tsx` 与相关测试类型；
- `test:i18n` 和阶段审计命令；
- 与锁文件和 `pnpm-workspace.yaml` 一致的依赖变化。

不得无审计地混入：

- `dev` 从 `next dev -p 3118` 改为 `next dev --turbo -p 3118`。该变化不属于中文化或发布必须条件，应在实际提交时单独保留／排除，不能借多语言提交夹带。

### 4.2 `frontend/src-tauri/tauri.conf.json` — `REVIEW_SECURITY_SENSITIVE_BUILD`

该文件同时包含以下有效发布内容：

- 英文和 `zh-CN` 模板资源；
- `dialog:allow-open`，供模板文件导入；
- NSIS 英文／简体中文选择器；
- `currentUser` 安装；
- `allowDowngrades=false`；
- NSIS Hook；
- WiX `upgradeCode`。

同时包含 CSP/`devCsp` 和 IPC 连接源变化。发布前必须结合真实升级测试和安全审核确认；特别是 `upgradeCode` 不能只靠静态检查证明与旧版本升级链兼容。

### 4.3 三个配套行为修复 — `REVIEW_SUPPORTING_CHANGE`

- `frontend/src/contexts/OnboardingContext.tsx`：避免状态尚未加载时自动保存并覆盖现有 Onboarding 状态；
- `frontend/src/hooks/usePlatform.ts`：未注册 OS 插件时安全回退到 User-Agent，不制造无意义 IPC 异常；
- `frontend/src/lib/navigation-guard.ts`：模板编辑器有未保存修改时延迟路由副作用。

三项均有回归测试或运行时证据，但应作为独立“配套修复”提交，不伪装成纯翻译变更。

## 5. 本轮验证

- i18n：54/54；
- 模板／导航 Node 兼容测试：41/41；
- TypeScript：0 错误；
- Next：13/13 页面；
- Rust：278 通过、0 失败、2 忽略；
- 强特征私钥／Token：0；
- 未分类路径：0。

本清单冻结的 298 文件边界已经按八组提交方案落实到 `codex/meetily-i18n-release-source`，并追加 `12f5f8c` 修复干净克隆测试对历史审计 JSON 的隐式依赖。`12f5f8c` 已在全新 worktree 通过离线安装、54/54 i18n、41/41 模板／导航、TypeScript、Next 13/13 和 Rust 278/0/2；这不等于已经生成正式 RC，也不授权覆盖 `target/release`。
