# Meetily 5A-1 提交拆分方案

本方案已经在 `codex/meetily-i18n-release-source` 上执行。前七组提交均由机器清单路径集合、逐文件 SHA-256 和 `git diff --cached --check` 三重门禁保护；第八组即承载本方案及紧凑审计产物的文档提交。正式产物仍未替换。

| 组 | 提交 | 结果 |
|---:|---|---|
| 1 | `de70fb5` | 仓库清理规则，1 文件 |
| 2 | `6595970` | 依赖与生产构建，8 文件；`--turbo` 已从提交索引排除 |
| 3 | `bf8916d` | 配套回归修复，3 文件 |
| 4 | `1d6b345` | 前端多语言与模板 UI，133 文件 |
| 5 | `11e7fea` | 原生 i18n 与模板引擎，39 文件 |
| 6 | `c614f73` | 自动测试，17 文件 |
| 7 | `27e3b44` | 审计工具，46 文件 |
| 8 | 本文档所在提交 | 架构、计划与 5A-1 紧凑审计，51 文件 |

干净 worktree 首次验收发现 2E／2F 测试依赖未提交生成证据，已追加 `12f5f8c test(i18n): make phase2 audits self-contained`。该修复只修改组 6 中既有的 2 个测试文件，分支相对基线的文件总数仍为 298。

收尾时发现共享工作区另一个模板任务在 5 个已提交文件上继续修改。为防止工作树字节污染冻结清单，追加 `c051ae2 fix(audit): pin inventory to release source ref`；外部修改未被暂存或覆盖。

当工作树状态继续扩展到 120 个快照外跟踪项时，追加 `d5669ae fix(audit): isolate pinned scope from worktree drift`，确保固定 Ref 清单不再并入任意当前 `trackedStatusPaths`。

## 提交 1：仓库清理规则

范围：

- `.gitignore`

验收：四个代表性本地产物路径均由明确规则忽略；展开的未跟踪文件由 216,215 降至源码／文档范围。

## 提交 2：依赖与生产构建

范围：

- `Cargo.lock`；
- `frontend/package.json`，但必须逐 Hunk 处理 `--turbo`；
- `frontend/pnpm-lock.yaml`；
- `frontend/pnpm-workspace.yaml`；
- `frontend/src-tauri/Cargo.toml`；
- `frontend/src-tauri/build.rs`；
- `frontend/src-tauri/tauri.conf.json`，需安全和升级审核；
- `frontend/src-tauri/scripts/nsis-installer-hooks.nsh`。

验收：锁文件与清单一致；生产构建通过；CSP、权限、UpgradeCode、降级策略和 NSIS Hook 均有明确审核记录。

## 提交 3：配套回归修复

范围：

- `frontend/src/contexts/OnboardingContext.tsx`；
- `frontend/src/hooks/usePlatform.ts`；
- `frontend/src/lib/navigation-guard.ts`。

验收：Onboarding 不被提前覆盖；无 OS 插件时安全回退；编辑器未保存状态能阻止导航副作用。

## 提交 4：前端多语言与模板 UI

范围：133 个文件。

包含：

- i18next Provider、资源与 Locale 解析；
- 英文／`zh-CN` 13 Namespace；
- 85 个既有 React 文件的迁移；
- 模板列表、编辑、导入、回收站、会议级选择和 Summary 接入。

验收：i18n 54/54，TypeScript 0，Next 13/13。

## 提交 5：原生 i18n 与模板引擎

范围：39 个文件。

包含：

- Rust 原生 Locale 和稳定错误结构；
- 托盘、通知和 Summary 原生文本；
- 模板 V2 Schema、Repository、Service、导入和快照；
- 6 个内置模板的英文／中文资源。

验收：Rust 278/0/2，模板结构和资源静态审计通过。

## 提交 6：自动测试

范围：17 个文件。

验收：i18n 54/54、Node 兼容模板／导航 41/41。两个 `bun:test` 文件留到 5A-2 统一处理。

## 提交 7：审计工具

范围：46 个文件，包括可重复脚本和三个隔离 Tauri 配置。

隔离配置只能用于审计，不能替代 `tauri.conf.json` 生成生产包。

## 提交 8：架构与审计文档

范围：51 个文件，包括计划、术语、基线、阶段结论和 4 份 5A-1 紧凑审计产物。其中 49 项进入机器清单，`source-inventory.json` 与 `sensitive-content-audit.json` 为避免自递归在清单外显式加入。历史生成截图／诊断 JSON 不在此提交内。

## 提交前门禁

每组实际提交前都必须：

1. 使用 `source-inventory.json` 校验路径和 SHA-256；
2. 不允许新增 `BLOCK_UNCLASSIFIED`；
3. 不允许强特征 Secret；
4. 五个显式审核文件必须逐项解决；
5. 完成提交后在干净 worktree 重建；
6. 新 RC 的 Git SHA、源清单哈希、EXE 哈希和安装包哈希必须建立双向关联。
