# 阶段 5 英文／简体中文端到端与回归报告

审计日期：2026-08-23
候选版本：0.4.0
审计平台：Windows 11 x64
自动化结论：`PASS`
完整发布门禁结论：参见《阶段 5 最终发布验收报告》

## 1. 审计对象

- 前端英文与简体中文资源：13 个 Namespace、每种语言 1578 个键；
- Tauri 原生资源：每种语言 33 个键；
- 会议总结模板：104 个冻结内容候选、6 个内置模板；
- 隔离候选程序：`target-phase5-release/release/meetily.exe`；
- 隔离候选安装包：`Meetily Phase 5 Audit_0.4.0_x64-setup.exe`；
- 正式冻结产物只做哈希核对，没有覆盖或安装。

## 2. 自动回归结果

| 检查 | 结果 | 数量／说明 | 原始证据 |
|---|---|---|---|
| 多语言测试 | PASS | 54/54 | `audit/phase-5-release/logs/frontend-i18n-tests.log` |
| Node 兼容前端库测试 | PASS | 41/41 | `audit/phase-5-release/logs/frontend-lib-tests.log` |
| TypeScript | PASS | 0 错误 | `audit/phase-5-release/logs/typescript-check.log` |
| Next 生产构建 | PASS | 13/13 静态页面 | `audit/phase-5-release/logs/next-production-build.log` |
| Rust 集成测试 | PASS | 278 通过、0 失败、2 忽略 | `audit/phase-5-release/logs/rust-app-lib-tests.log` |
| 静态多语言审计 | PASS | 13/13 | `audit/phase-5-release/static-audit.json` |
| WebView2 运行时审计 | PASS | 16/16 | `audit/phase-5-release/runtime/runtime-audit.json` |
| Windows 安装链路 | PASS | 17/17 | `audit/phase-5-release/windows-install-audit.json` |
| 候选／正式产物完整性 | PASS | 6/6 | `audit/phase-5-release/release-integrity.json` |

两个既有测试文件直接导入 `bun:test`：`blocknote-markdown.test.ts` 与 `summary-language-preferences.test.js`。本机未安装 Bun，因此它们没有被计入 Node 兼容的 41/41；该限制已作为 `P5-TOOL-001` 登记，不能表述为“所有前端测试均已执行”。

## 3. 英文／中文运行时结果

| 场景 | 英文 | 简体中文 | 结果 |
|---|---|---|---|
| 设置页加载与 `<html lang>` | `en` | `zh-CN` | PASS |
| 总结模板列表 | 6 个内置模板 | 6 个内置模板 | PASS |
| 运行时立即切换 | 无需重启 | 无需重启 | PASS |
| 离线加载语言和模板资源 | 已由英文资源基线覆盖 | 禁网后 `zh-CN` 与 6 模板可用 | PASS |
| 可见原始翻译键 | 0 | 0 | PASS |
| Unicode 替换字符 | 0 | 0 | PASS |
| 可见控件无名称 | 0 | 0 | PASS |
| 50 次语言切换 | 最终状态一致 | 最终 UI／原生 Locale 均为 `zh-CN` | PASS |
| 转录配置冒泡 | 未改变 | 未改变 | PASS |
| 模型配置冒泡 | 未改变 | 未改变 | PASS |
| 录音状态冒泡 | 未改变 | 未改变 | PASS |
| 运行时异常／错误日志／控制台错误 | 0/0/0 | 0/0/0 | PASS |

## 4. 不能由本轮自动化替代的场景

以下场景没有足够证据，状态为 `OPEN`，不能推断为通过：

- 真实麦克风和系统音频的权限允许、拒绝与稍后处理；
- 60 分钟录音期间暂停、继续、Locale 切换、停止和保存；
- Windows Shell 中英文托盘和通知的人工截图与交互；
- MP3/WAV/MP4 的真实导入、取消和重新转录长链路；
- Whisper、Parakeet 和摘要模型的真实下载、取消、重试、删除；
- 多 Provider 的中英文真实摘要质量及 Prompt 服从性人工复核；
- 在线更新的有更新、无更新、失败与重试；
- Narrator／NVDA 的真实读屏和全键盘操作。

## 5. 结论边界

本报告证明本轮可自动化的中英文资源、核心状态隔离、构建和运行时检查通过；它不证明硬件、真实模型、真实外部服务、辅助技术和长时稳定性已经通过。最终发布必须以阶段 5 发布门禁报告为准。
