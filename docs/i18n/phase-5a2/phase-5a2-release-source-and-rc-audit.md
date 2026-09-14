# Meetily 阶段 5A-2 发布源码合并与 Windows RC 严格验收报告

- 审计日期：2026-08-23
- 阶段：15.10 阶段 5A-2——批准变更合并、干净 RC 与安装链路复验
- 自动化技术结论：**PASS**
- 生产发布结论：**FAIL／NO-GO（仍有签名、治理、真实设备和真实历史数据等门禁）**

## 1. 结论与边界

阶段 5A-2 已把稳定的会议模板 JSON 导入／导出功能合并到阶段 5A-1 的干净发布源码，关闭 Node 单测与非交互 ESLint 两项工具覆盖缺口，并从唯一干净提交生成独立 Windows RC。RC 已完成全新安装、启动、同版本修复、默认卸载、数据保留、0.3.9 合成旧版升级到 0.4.0、静默降级阻止和审计后清理。

本报告只批准“源码可追溯、自动化回归、隔离 RC 构建和本机 NSIS 语义”子门禁。候选 EXE、安装包和安装后 Payload 均为 `NotSigned`；合成 0.3.9 夹具不能证明真实用户的会议数据库、录音、模型、自定义模板和历史设置已经完成跨版本迁移。因此不得把本阶段的 `PASS` 解释为生产正式发布 `PASS`，也不得覆盖冻结的 `target/release` 正式二进制。

## 2. 源码与并发工作区隔离

| 验收项 | 标准 | 实际结果 | 结论 |
|---|---|---|---|
| 发布基线 | 只从阶段 5A-1 已验证提交继续 | 基线 `f2eb887bfd0848803c4287d4f2da07f27df111a5` | PASS |
| 共享工作区稳定性 | 目标文件连续采样哈希一致 | 10 个目标文件双采样、间隔 8 秒，10/10 一致 | PASS |
| 变更边界 | 只纳入已审核的会议模板 JSON 导入／导出文件 | 10 个目标文件；约 110 个并发 Rust 格式化漂移全部排除 | PASS |
| 字节一致性 | 隔离工作树目标文件与冻结输入一致 | 10/10 规范化字节一致 | PASS |
| 合并提交 | 功能变更具有独立提交 | `ea4b74fafd41f5932f3c82f410a447685b133e60` | PASS |
| 正式产物保护 | 不写入 `target/release` 二进制 | 审计前后正式 EXE／安装包哈希均为冻结值 | PASS |

纳入的 10 个文件：

1. `frontend/src-tauri/src/lib.rs`；
2. `frontend/src-tauri/src/summary/template_commands_v2.rs`；
3. `frontend/src-tauri/src/summary/templates/service.rs`；
4. `frontend/src/components/templates/TemplateImportDialog.tsx`；
5. `frontend/src/components/templates/TemplateLibraryPage.tsx`；
6. `frontend/src/i18n/locales/en/templates.json`；
7. `frontend/src/i18n/locales/zh-CN/templates.json`；
8. `frontend/src/services/templateService.ts`；
9. `frontend/src/types/summary-template.ts`；
10. `frontend/tests/lib/template-import.test.ts`。

## 3. JSON 导入／导出安全与功能审计

| 领域 | 验收标准 | 结果 |
|---|---|---|
| 输入大小 | 单文件最大 1 MiB，超限明确拒绝 | PASS |
| 文本编码 | UTF-8；允许 UTF-8 BOM；非法编码拒绝 | PASS |
| 语法错误 | 返回行列位置，不回显可能敏感的原文 | PASS |
| 版本兼容 | V1 可迁移到 V2；V2 必须通过 Schema／业务校验 | PASS |
| 文件写入 | 临时文件持久化后原子替换；拒绝符号链接目标 | PASS |
| 批量导入 | 单次最多 50 个文件；逐文件错误隔离 | PASS |
| 前端隐私 | 错误提示只显示文件基本名，不暴露完整本机路径 | PASS |
| 多语言 | 英文／简体中文键集合与占位符一致 | PASS |
| 测试 | TypeScript JSON 导入 4/4；新增 Rust 场景计入 283 项通过 | PASS |

## 4. 工具链门禁关闭

阶段 5 的 `P5-TOOL-001` 和 `P5-TOOL-002` 已关闭：

- 两个 `bun:test` 测试迁移到 Node `node:test`／`assert`，移除 `@types/bun`；
- 新增 `test:unit` 和 `test:all`，完整 Node 单元测试为 51/51，多语言测试为 54/54，总计 105/105；
- 新增非交互 `pnpm lint`，最终 0 error／0 warning；
- 对全仓遗留的 7 类规则建立透明冻结基线，并在发布关键路径重新以 error 启用，避免把 219 个历史错误和 43 个历史警告伪装成新增缺陷；
- 工具链提交：`2f274376a4b772b04dc0919666806714d0cec3e6`。

## 5. 干净 RC 构建与供应链追溯

RC 构建提交为 `4edc53116b5d5e186d306dccdc5b45729777f487`。构建开始时 Git 已跟踪状态为 0、未忽略未跟踪状态为 0；构建后同样为 0。841 个构建输入中没有文件晚于候选 EXE。构建命令明确使用 `--no-sign --ci`，没有读取签名私钥变量。

| 产物／输入 | 字节 | SHA-256 | 状态 |
|---|---:|---|---|
| RC EXE | 65,776,128 | `D4C8E31D9504B25DC8C77DC30C0C5A506055E2C20BE082347B763901AA640D84` | NotSigned、审计候选 |
| RC NSIS | 43,996,335 | `5CFE9C41360EC135C407FF8FC8D00EE23BAE46CB2E1E8632DAC51EE91BA61FBE` | NotSigned、审计候选 |
| `llama-helper` | 3,773,440 | `EE4E303B64603DFF8369644EDF3792ADD1948605C62F3BEC66A2549757EF97EC` | 从当前源码 `--locked --release` 构建 |
| FFmpeg 8.0.1 | 99,264,000 | `5AF82A0D4FE2B9EAE211B967332EA97EDFC51C6B328CA35B827E73EAC560DC0D` | 构建脚本缓存并版本校验 |
| 正式冻结 EXE | — | `1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823` | 未覆盖 |
| 正式冻结安装包 | — | `C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434` | 未覆盖 |

`llama-helper` 已通过 `ping → pong` 和 `shutdown → goodbye` 协议烟雾测试。安装后的 helper 与 FFmpeg 哈希分别精确匹配上述构建输入。

## 6. 自动回归验收

| 门禁 | 结果 | 判定 |
|---|---:|---|
| ESLint | 0 error／0 warning | PASS |
| Node 单元测试 | 51/51 | PASS |
| 多语言测试 | 54/54 | PASS |
| TypeScript | 0 error | PASS |
| Next 生产构建 | 13/13 页面 | PASS |
| Rust | 283 passed／0 failed／2 ignored；Doc-test 1/1 | PASS |
| RC 发布构建 | Exit 0；NSIS 1/1 | PASS |

两项 Rust ignored 测试为既有显式忽略，不是失败。测试期通过 `TAURI_CONFIG` 仅排除 sidecar 文件存在性检查；正式 RC 构建实际携带并验证真实 sidecar。

## 7. Windows 安装、升级、降级与卸载审计

候选使用独立产品名 `Meetily Phase 5A2 RC`、Bundle ID `com.meetily.ai.phase5a2rc` 和独立 LocalAppData 安装目录，不安装、升级或卸载正式 Meetily。

### 7.1 全新安装链路

机器报告：`docs/i18n/audit/phase-5a2/windows-install-audit.json`。

- 19/19 断言通过；
- 清洁安装返回 0，注册表名称、0.4.0 版本和安装位置正确；
- 安装后应用存活超过 5 秒；
- helper／FFmpeg 哈希与构建输入一致；
- 同版本修复返回 0，应用数据哨兵哈希不变；
- 默认卸载返回 0，注册项和程序目录移除，应用数据按默认策略保留；
- 审计 finally 清理后，隔离注册项、程序目录和隔离 AppData 均不存在；
- 用户录音和共享模板位置未触碰。

### 7.2 合成升级与降级阻止链路

机器报告：`docs/i18n/audit/phase-5a2/windows-upgrade-downgrade-audit.json`。

- 21/21 断言通过；
- 同一隔离身份的合成 0.3.9 安装成功，安装包 SHA-256 为 `939A6DD29983C453CBC62A9F44C0D61CC3ADB7C3D956A15A60414311C6FCBADC`；
- 原位升级到冻结的 0.4.0 RC 返回 0，注册版本和 Payload 版本均为 0.4.0；
- 数据哨兵在升级前后哈希一致，升级后应用可启动；
- 使用 0.3.9 尝试静默降级返回预期退出码 3；注册版本仍为 0.4.0，安装后 EXE 哈希和数据哨兵均不变；
- 升级后版本默认卸载成功，程序清理和数据保留策略正确；
- 最终隔离清理完整，正式产物哈希不变。

该链路证明 `allowDowngrades: false` 及 NSIS hook 的安装器语义正确，但 `syntheticFixture=true`、`closesHistoricalDataMigrationGate=false`。`P5-UPG-001` 继续保持 OPEN，直至用批准的真实历史版本和脱敏旧用户数据副本完成升级、回滚与内容可读性验证。

## 8. 缺陷与发布门禁处置

| ID | 5A-2 处置 | 状态 |
|---|---|---|
| P5-REL-001 | 干净提交 `4edc531` 生成唯一可追溯 RC，构建前后 Git 状态 0 | CLOSED |
| P5-TOOL-001 | Bun 测试迁移到 Node，完整 Node 测试 51/51 | CLOSED |
| P5-TOOL-002 | 非交互 ESLint 建立，0 error／0 warning | CLOSED |
| P5-UPG-001 | 合成安装器升级／降级语义通过，但真实历史数据升级和批准回滚包未覆盖 | OPEN |
| P5-SIGN-001 | RC、安装包和 Payload 均为 NotSigned | OPEN／生产阻断 |
| 其他治理／设备门禁 | 语言与法律签字、平台范围、60 分钟真机录音、真实 Provider、性能基线、读屏、原生 Shell 证据未完成 | OPEN／生产阻断 |

## 9. 最终验收矩阵

| 审计部分 | 必须满足的验收标准 | 结果 |
|---|---|---|
| 变更输入 | 稳定、边界明确、无并发漂移混入 | PASS |
| 安全实现 | 大小、编码、Schema、原子写入、路径隐私和批量上限全部生效 | PASS |
| 工具链 | 全部 Node 测试可运行，Lint 非交互且新增关键路径零告警 | PASS |
| 构建追溯 | 干净提交、真实 sidecar、候选哈希和构建输入可对应 | PASS |
| Windows 新装 | 安装、启动、修复、Payload、卸载、保留和清理全部通过 | PASS |
| 安装器升级语义 | 合成旧版升级、降级阻止、数据哨兵和卸载通过 | PASS |
| 真实历史数据迁移 | 批准的历史版本和脱敏真实数据升级／回滚可读性 | OPEN |
| 代码签名 | 生产证书、时间戳、签名链和 SmartScreen 验证 | OPEN |
| 最终治理 | 所有签字、平台、设备、AI、性能、可访问性和原生 Shell 证据齐全 | OPEN |

因此，阶段 5A-2 的自动化和隔离 RC 验收为 **PASS**；Meetily 中文生产发布门禁仍为 **FAIL／NO-GO**。

## 10. 证据索引

- `docs/i18n/phase-5a2/release-candidate-manifest.json`；
- `docs/i18n/phase-5a2/windows-install-upgrade-uninstall-report.md`；
- `docs/i18n/audit/phase-5a2/windows-install-audit.json`；
- `docs/i18n/audit/phase-5a2/windows-upgrade-downgrade-audit.json`；
- `docs/i18n/scripts/audit-phase5a2-windows-install.ps1`；
- `docs/i18n/scripts/audit-phase5a2-windows-upgrade.ps1`。
