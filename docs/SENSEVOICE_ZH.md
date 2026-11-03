# SenseVoice 本地转写使用说明

## 模型与适用场景

本版加入 SenseVoiceSmall 的 int8 ONNX 模型，界面名称为 **SenseVoice**，模型标识为 `sensevoice-small-int8`。模型与词表合计约 239.55 MB（约 228.45 MiB）。支持中文、粤语、英文、日文和韩文，可选择自动识别语言。

采用 [SenseVoice 项目](https://github.com/QwenAudio/SenseVoice/blob/main/README_zh.md)公开模型的 [sherpa-onnx 转换版本](https://k2-fsa.github.io/sherpa/onnx/sense-voice/pretrained.html)。它负责转写文字，本次没有接入说话人分离、参会者真实姓名识别或翻译功能。情感和声音事件标签不写入会议正文。

## 如何使用

1. 新安装首次设置时，默认使用 SenseVoice 转写模型。先选择下载来源，再点击“下载 SenseVoice”。纪要模型不随安装自动下载，也不会在点击“继续”后偷偷后台下载；进入软件后在“设置 → 纪要”选择本地模型并下载，或配置自己的 AI 接口。
2. 已完成设置的用户，原有 Whisper / Parakeet 选择会保留。需要使用新模型时，打开 **设置 → 转写模型**，选择 **SenseVoice（推荐 · 中文与多语言 · 本地）**，下载并选中模型。
3. 普通话会议可选择“中文”，多语言会议可选择“自动识别”。粤语可单独选择。
4. 录音、导入音频和重新转写都可以使用 SenseVoice。已经排队或暂停的旧任务保留提交时的模型设置，不会自动改用新模型。
5. 如需用新模型重做历史会议，在该会议的重新转写窗口选择 SenseVoice 后提交。

更换转写模型不会改变纪要生成的 AI 接口，也不会改变确认后发送到 Lark 的流程。

## 模型目录与下载

文件保存在已配置的模型根目录下：

```text
<模型根目录>/sensevoice/sensevoice-small-int8/model.int8.onnx
<模型根目录>/sensevoice/sensevoice-small-int8/tokens.txt
```

沿用现有模型目录选择和迁移功能。下载支持断点续传、取消、重试与备用源；跨下载源切换时保留同一个下载任务，防止取消后又启动下载或并发写入。如果下载服务器返回不规范的断点响应，会重新请求当前文件的完整内容，已完整下载且校验通过的其他文件会保留。

首次设置和“设置 → 转写”中的 SenseVoice 页面均提供“下载来源”选项，并记住上次选择：

- **自动切换（推荐）**：依次尝试 Hugging Face、ModelScope、HF 镜像站。
- **Hugging Face**：仅使用 `huggingface.co` 上的转换模型，固定版本 `2365baeacb507f821a0c8120fcee3d484dba7a07`。
- **ModelScope（国内）**：仅使用 `modelscope.cn` 的社区镜像。
- **HF 镜像站（国内）**：仅使用第三方 `hf-mirror.com`，保持与 Hugging Face 相同的固定版本路径。

手动选择单个来源时不会静默切换到其他站点。下载中若要换源，先点击“取消下载”，等待取消完成，再选择其他来源并重试。来源只影响下载地址，不改变模型保存目录。

2026-11-03 修复：下载请求增加正常的 Meetily 客户端标识（User-Agent），解决 ModelScope CDN 对缺少客户端标识的请求返回 `403 / denied by UA ACL = blacklist` 的问题。Hugging Face 或 HF 镜像站出现连接重置时，可手动选择 ModelScope 重试；该修复不保证所有网络都能访问全部来源。

Hugging Face 与 HF 镜像站使用 `csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17`；ModelScope 使用 `WEAAEW/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17`。ModelScope 文件已与 sherpa-onnx 发布归档核对。各站点可用性取决于网络，程序对所有来源固定校验以下文件内容，镜像替换文件后若校验不符，会拒绝使用：

| 文件 | 字节数 | SHA-256 |
| --- | ---: | --- |
| model.int8.onnx | 239233841 | c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51 |
| tokens.txt | 315894 | f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc |

模型授权以原始发布方为准；转换归档中的 LICENSE 指向 [FunASR 授权说明](https://github.com/modelscope/FunASR#license)。安装包不内嵌模型权重。

## 本地处理与长录音

推理复用程序已有的 ONNX Runtime，特征提取使用固定版本 `kaldi-fbank 0.1.0`。不安装 Python，不执行下载仓库的脚本，不调用 SenseVoice 云端识别服务。模型下载会访问上述下载源；此转写实现不向下载源发送录音或文字。

长录音沿用现有语音活动检测、静音附近分段和重叠衔接处理。以 25 秒为目标分段，单次模型推理拒绝超过 30 秒的输入；实时路径收到超长语音段时也会先分段。保留后台队列、暂停及已保存分段继续处理的能力。这限制单次推理内存，不意味着完整音频解码、排队文件及程序其他部分完全不占用内存。

## 验证与交付范围

Windows 上使用公开的中文、粤语、英文、日文和韩文短音频进行了真实本地模型推理验证。实际下载测试保留已验证的模型文件、将词表截成 128 字节后调用正式下载实现，验证备用源恢复、SHA-256 校验、模型加载和中文转写；另外用超过 30 秒的连续音频验证自动分段。测试全部在独立临时目录运行，没有使用用户会议录音。

下载与模型管理 14 项回归通过，包含来源白名单、单来源选择、断点续传和取消并发保护。本次新增 4 项首次设置回归，验证安装时不下载纪要模型、完成设置不触发隐式下载、失败换源和取消后换源。此前的分段衔接 6 项回归、旧首次设置数据兼容测试，以及模型选择、后台任务提交、编辑器粘贴安全测试通过。短样本验证不代表真实会议准确率保证，建议用一段已知内容的会议音频比较识别效果。

本次 Windows 安装包同时包含编辑器粘贴安全修复。macOS 共用同一套源代码，但需要重新运行 Mac 云端构建并在 Mac 上验证；此前生成的 DMG 不会自动包含本次变更。
