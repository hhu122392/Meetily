# 第三方组件说明

本分支保留 [Meetily 原项目的 MIT 许可证](LICENSE.md) 和 Zackriya Solutions 的版权声明。新增代码沿用项目 MIT 许可；这不改变第三方代码和模型自身的授权条款。

下表是主要组件指引，不是完整传递依赖清单。实际版本以 `frontend/pnpm-lock.yaml` 与 `Cargo.lock` 为准。

| 组件 | 许可与源码位置 |
| --- | --- |
| BlockNote core / react / shadcn 0.36.0 | MPL-2.0；[对应版本源码](https://github.com/TypeCellOS/BlockNote/tree/v0.36.0/packages)，[许可文件](licenses/BlockNote-MPL-2.0.txt) |
| Tauri | MIT 或 Apache-2.0；https://github.com/tauri-apps/tauri |
| ONNX Runtime | MIT；https://github.com/microsoft/onnxruntime |
| llama.cpp | MIT；https://github.com/ggml-org/llama.cpp |
| whisper.cpp | MIT；https://github.com/ggml-org/whisper.cpp |
| FFmpeg | 许可取决于具体构建选项；https://ffmpeg.org/legal.html 。构建下载入口见 `frontend/src-tauri/build/ffmpeg.rs`，对外分发二进制时需另行核对对应许可证和源码提供义务。 |

BlockNote 受 MPL-2.0 保护的代码保留其许可；对外分发包含它的安装包时，应告知接收者对应源码的获取位置。本分支没有将 BlockNote 的源码改为 MIT 许可。

SenseVoice、Parakeet、Whisper 与 Qwen 模型权重未包含在此源码快照中。应用提供下载功能；使用和再次分发权重时须核对对应模型卡和许可证。

本说明保留许可证原文以避免改变授权含义。完整依赖审计、商标权或公司代码权属审核不在此次源码上传的验证范围内。
