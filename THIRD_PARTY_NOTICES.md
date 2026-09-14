# Third-party components

Meetily source is MIT licensed; retain LICENSE.md and the upstream copyright notice.

- Tauri: MIT / Apache-2.0 — https://github.com/tauri-apps/tauri
- sherpa-onnx 1.13.7 and the bundled speech C API: Apache-2.0 — https://github.com/k2-fsa/sherpa-onnx/tree/v1.13.7
- ONNX Runtime: MIT — https://github.com/microsoft/onnxruntime
- llama.cpp: MIT — https://github.com/ggml-org/llama.cpp (the exact binding revision is in Cargo.lock).
- Microsoft Visual C++ runtime and DirectML: Microsoft redistributable terms; inventories are under frontend/src-tauri/runtime/windows-x64/.
- Microsoft WebView2 Fixed Version Runtime: Microsoft terms; version and verified download are pinned in frontend/src-tauri/runtime/webview2-fixed.lock.json. Its supplied notices remain in the packaged runtime.
- FFmpeg is a separate command-line executable. The bundled 8.0.1 build reports GPLv3 features; its license text is included under licenses/. Source: https://ffmpeg.org/releases/ffmpeg-8.0.1.tar.xz ; Windows build source and dependency/build scripts: https://github.com/GyanD/codexffmpeg and https://github.com/m-ab-s/media-autobuild_suite . The binary is obtained from the existing pinned upstream release referenced in frontend/src-tauri/build/ffmpeg.rs.

SenseVoice and Qwen model weights are not redistributed inside this installer. The application downloads the model selected by the user from its listed source. Their model cards and licenses apply separately.
