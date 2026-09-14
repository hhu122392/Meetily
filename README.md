# Meetily 会议助手

面向 Windows 的开源会议录音、转录和摘要客户端。本仓库基于 [Zackriya Solutions / Meetily](https://github.com/Zackriya-Solutions/meeting-minutes) 修改，保留原项目 MIT 许可。

## 下载使用

到 [Releases](https://github.com/hhu122392/Meetily/releases) 下载 `Meetily-0.4.2-windows-x64-setup.exe`，双击安装。无需安装 Python、Rust、Node.js 或单独配置 FFmpeg。安装包内置 WebView2 和必需的音频、推理运行库。

1. 适用于 Windows 10/11 x64；ARM64、32 位及其他系统未在本版验收范围内。
2. 首次打开，在准备页下载 SenseVoice 转录模型和选定的本地摘要模型。模型没有包含在安装包里；下载完成后可本地使用。需要预留数 GB 磁盘空间。
3. 选择麦克风和系统声音后开始录音，也可导入已有音频。
4. 会后选择模板生成摘要。使用 DeepSeek 等 API 时自行填写密钥，转录文字会发送到所选服务；本地模型模式不需要 API 密钥。

本仓库和 Release 已公开，无需登录 GitHub 即可下载。此社区构建未使用发布者签名，Windows 可能提示“未知发布者”。请从本仓库下载，并用同一 Release 的 `SHA256SUMS.txt` 核对文件。

## 当前功能

- SenseVoice 中文优先本地转录、实时文本更新和录音文件保存。
- 转录编辑、修订记录、来源回查及模板化会议摘要。
- 本地 Qwen 摘要和 OpenAI 兼容 API 摘要。
- 摘要后台任务恢复，人工修改保留生成原稿。
- 普通说明收纳在问号提示中。

## 已知限制

- 本地小模型仍可能混淆决定、条件、负责人和期限。重要摘要应按原文核对；目前不能宣称内容准确率已全面通过。
- SenseVoice 本身不能可靠地把声音对应到具体姓名；没有身份依据的内容不能推断负责人。
- 自动更新暂不启用；“查看发布版本”打开本仓库 Releases。
- 模型下载需要网络，速度取决于模型源和网络；商业 API 另行计费。

## 开发构建

需要 Windows x64、Visual Studio C++ Build Tools 和 Windows SDK、Rust MSVC 工具链、Node.js 22+、Corepack/pnpm，以及可供 bindgen 使用的 LLVM/libclang（设置 `LIBCLANG_PATH`）。

```powershell
git clone --recurse-submodules https://github.com/hhu122392/Meetily.git
cd Meetily
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
.\scripts\build-community-release.ps1
```

构建脚本下载前端依赖、准备辅助程序及固定版本 WebView2，再生成 NSIS 安装包；模型不属于编译依赖。详见 [发行规则](docs/releases/community-distribution.md)、[第三方组件](THIRD_PARTY_NOTICES.md) 和 [发布说明](docs/releases/v0.4.2.md)。

`backend/` 是保留的旧 Python 服务源码，当前桌面默认流程不要求运行它。原交接目录中的个人测试记录、录音、旧二进制和本机环境文件不属于此源码发行。

## 许可

[MIT License](LICENSE.md)。第三方运行库、模型和 FFmpeg 使用各自许可证；本仓库不是原项目商业版本。
