# Meetily 中文 Lark 定制版

本分支是基于 [Zackriya Solutions / Meetily](https://github.com/Zackriya-Solutions/meetily) 的第三方定制源码，保留原项目的 [MIT 许可证](LICENSE.md)。不是 Meetily 或 Lark 官方发行版。

## 分支与版本

- 分支：`codex/summary-templates-layout-20261107`。
- 应用版本：0.4.1；本轮功能与文档更新日期：2026-11-07。
- 本分支保存 Windows x64 和 Mac Apple Silicon 定制源码，不包含安装包、模型权重、个人录音、会议数据库或运行凭证。分支后续提交不代表已更新安装包；安装包应按构建提交核对。
- 仓库 `main` 是另一条独立定制路线。本次上传没有合并或覆盖它；版本数字不能直接用于比较两条路线的功能。
- 此分支以独立源码快照发布，不携带原私有仓库的提交历史。

## 定制功能

- 中文界面，SenseVoiceSmall 本地中文与多语言转写，可选择模型下载来源。
- 修复 ModelScope 下载因缺少 User-Agent 返回 403 的问题。
- 首次设置可选择是否下载本地纪要模型，也可使用自行配置的 AI 接口。
- Lark 企业应用、授权、群会议匹配与会议助手流程。
- 纪要可编辑，发送前预览并确认，支持概要卡片和可选 Markdown 附件。
- 按公司隔离本地会议资料，支持录音与模型目录选择、后台转写与暂停恢复。
- 支持 Windows x64 与 Mac Apple Silicon 构建。
- 长时间生成纪要以后台实际状态为准，不再因前端轮询次数达到上限而显示超时失败。
- 按会议保存背景、模板、出席人员、缺席人员和主持人，生成时固定本次输入与模板快照。
- Markdown 历史版本可查看并确认恢复，恢复前备份当前文件，不自动发送到 Lark。

## Lark 部署与本轮更新

- [Lark 权限申请清单](docs/Lark权限申请清单.md)：可直接提交给新公司管理员，区分应用身份、用户身份、事件和回调。
- [Lark 与 Meetily 配置及变更说明](docs/Lark与Meetily配置及变更说明.md)：新公司接入、已有应用升级、Mac 配置及逐步验收。
- [权限批量导入 JSON](docs/lark-permissions-import.json)／[权限与事件核对清单](docs/lark-permissions-checklist.json)。
- [Mac 安装与使用说明](docs/Mac安装与使用说明-20261107.md)。
- [纪要模板与布局移植](docs/纪要模板与布局移植.md)、[转写文字纠错与逐段编辑](docs/转写文字纠错移植.md)。

本轮新增中英文模板库和模板管理，整理纪要及 Lark 工具栏，并加入规则／AI 文字纠错、备份恢复和逐段手动编辑。这些新增功能本身不需要额外的 Lark 权限。切换模板或保存转写后，已有纪要需重新生成才会更新。

## 构建与配置

前端依赖版本由 `frontend/pnpm-lock.yaml` 固定，Rust 依赖由 `Cargo.lock` 固定。使用 Node.js 20、pnpm 9.15.9 和 Rust 工具链。Windows 还需 Visual Studio C++ 工具及 Windows SDK；Mac 需要 Xcode 命令行工具和 CMake。

Mac 本机构建步骤见 [Mac 构建说明](docs/MACOS_CUSTOM_BUILD_ZH.md)。工作流位于 `.github/workflows/build-macos-custom.yml`，只手动触发；GitHub 手动触发入口要求工作流也存在于默认分支，本次上传不会为此改动 `main`。

- [SenseVoice 配置与下载说明](docs/SENSEVOICE_ZH.md)
- [纪要模型下载说明](docs/SUMMARY_DOWNLOAD_ZH.md)
- [编辑器粘贴安全修复说明](docs/EDITOR_CLIPBOARD_SECURITY_ZH.md)
- [三项功能移植说明与验证](docs/PORT_044_FEATURES_ZH.md)
- [第三方组件说明](THIRD_PARTY_NOTICES.md)

Lark App ID、App Secret 和个人授权须由使用者自行配置。不要把真实凭证、Webhook、会议资料或日志提交到仓库。使用第三方 AI 接口生成纪要时，转写文字会发送到用户配置的服务。

## 验证与已知限制

本次发布前已完成 Windows 覆盖安装、ModelScope 全新模型下载和本地中文推理验证。Mac ARM64 云端构建、20 项选定后端测试及应用和 DMG 完整性检查通过；2 项依赖实际模型或网络的测试未在 Mac 云端运行。

Mac 要求 macOS 14.4 或以上；定制包未做 Apple Developer ID 签名与公证，麦克风、系统声音、模型推理和 Lark 流程仍需在真实设备验收。

下载提示修复：收到完成状态后主动关闭浮窗；99% 时以本机模型就绪状态兜底，不以字节数直接判断成功。已加入完成、失败重试、取消和卸载的回归测试。

## 许可

保留原项目版权及 MIT 许可证。依赖、字体、模型与构建时下载的组件按各自许可证使用；本分支公开不代表完成了完整法律或依赖合规审计。
