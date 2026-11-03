# Meetily 中文 Lark 定制版

本分支是基于 [Zackriya Solutions / Meetily](https://github.com/Zackriya-Solutions/meetily) 的第三方定制源码，保留原项目的 [MIT 许可证](LICENSE.md)。不是 Meetily 或 Lark 官方发行版。

## 分支与版本

- 分支：`codex/lark-desktop-20261103`。
- 应用版本：0.4.1；源码快照日期：2026-11-03。
- 本分支对应本次 Windows x64 和 Mac Apple Silicon 定制安装包的源码，不包含安装包、模型权重、个人录音、会议数据库或运行凭证。
- 仓库 `main` 的 0.4.4 是另一条独立定制路线。本次上传没有合并或覆盖它；版本数字不能直接用于比较两条路线的功能。
- 此分支以独立源码快照发布，不携带原私有仓库的提交历史。

## 定制功能

- 中文界面，SenseVoiceSmall 本地中文与多语言转写，可选择模型下载来源。
- 修复 ModelScope 下载因缺少 User-Agent 返回 403 的问题。
- 首次设置可选择是否下载本地纪要模型，也可使用自行配置的 AI 接口。
- Lark 企业应用、授权、群会议匹配与会议助手流程。
- 纪要可编辑，发送前预览并确认，支持概要卡片和可选 Markdown 附件。
- 按公司隔离本地会议资料，支持录音与模型目录选择、后台转写与暂停恢复。
- 支持 Windows x64 与 Mac Apple Silicon 构建。

## 构建与配置

前端依赖版本由 `frontend/pnpm-lock.yaml` 固定，Rust 依赖由 `Cargo.lock` 固定。使用 Node.js 20、pnpm 9.15.9 和 Rust 工具链。Windows 还需 Visual Studio C++ 工具及 Windows SDK；Mac 需要 Xcode 命令行工具和 CMake。

Mac 本机构建步骤见 [Mac 构建说明](docs/MACOS_CUSTOM_BUILD_ZH.md)。工作流位于 `.github/workflows/build-macos-custom.yml`，只手动触发；GitHub 手动触发入口要求工作流也存在于默认分支，本次上传不会为此改动 `main`。

- [SenseVoice 配置与下载说明](docs/SENSEVOICE_ZH.md)
- [纪要模型下载说明](docs/SUMMARY_DOWNLOAD_ZH.md)
- [编辑器粘贴安全修复说明](docs/EDITOR_CLIPBOARD_SECURITY_ZH.md)
- [第三方组件说明](THIRD_PARTY_NOTICES.md)

Lark App ID、App Secret 和个人授权须由使用者自行配置。不要把真实凭证、Webhook、会议资料或日志提交到仓库。使用第三方 AI 接口生成纪要时，转写文字会发送到用户配置的服务。

## 验证与已知限制

本次发布前已完成 Windows 覆盖安装、ModelScope 全新模型下载和本地中文推理验证。Mac ARM64 云端构建、20 项选定后端测试及应用和 DMG 完整性检查通过；2 项依赖实际模型或网络的测试未在 Mac 云端运行。

Mac 要求 macOS 14.4 或以上；定制包未做 Apple Developer ID 签名与公证，麦克风、系统声音、模型推理和 Lark 流程仍需在真实设备验收。

已知界面问题：模型显示“就绪”后，右上角下载浮窗仍可能残留 99%。本快照没有修复这一提示同步问题，不应据此重复下载模型。

## 许可

保留原项目版权及 MIT 许可证。依赖、字体、模型与构建时下载的组件按各自许可证使用；本分支公开不代表完成了完整法律或依赖合规审计。
