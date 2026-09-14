# Windows System Audio Idle Start Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkboxes for tracking.

**Goal:** 修复 Windows 输出设备空闲两秒就被误判为系统音频不可用的问题，并给真正的设备错误提供一键进入设置的入口。

**Architecture:** 把 WASAPI“接口已经启动”和“已经收到首批声音”拆成两个状态。后台在 `IAudioClient::Start()` 成功后立即返回启动成功，静默期间保持流存活；前端用回调计数显示等待状态，真实初始化失败才显示设备设置入口。

**Tech Stack:** Rust、Windows WASAPI、Tauri 2、React、TypeScript、i18next、Node test runner

---

### Task 1: 建立失败回归测试

**Files:**
- Modify: `frontend/src-tauri/tests/i10a_windows_audio_red.rs`
- Modify: `frontend/tests/lib/i10a-frontend-contract.test.ts`

- [x] **Step 1: 写 Windows 后台失败测试**

增加契约测试，要求启动确认出现在 `IAudioClient::Start()` 与 `capture_loop()` 之间，并禁止“首个回调超过两秒就启动失败”的错误路径：

```rust
#[test]
fn idle_output_endpoint_is_ready_after_wasapi_start_without_waiting_for_audio() {
    let start = WINDOWS_LOOPBACK.find(".Start()").unwrap();
    let capture = WINDOWS_LOOPBACK[start..].find("capture_loop(").unwrap() + start;
    let ready = WINDOWS_LOOPBACK[start..].find("sender.send(Ok(()))").unwrap() + start;
    assert!(start < ready && ready < capture);
    assert!(!WINDOWS_LOOPBACK.contains("produced no callback within 2 seconds"));
    assert!(!WINDOWS_LOOPBACK.contains("first callback took at least 2 seconds"));
}
```

- [x] **Step 2: 写前端失败测试**

增加契约测试，要求录音状态栏根据 `callback_count === 0` 显示 `status.waitingForSystemAudio`，错误卡片提供 `actions.openAudioDeviceSettings`，中英文录音文案不再包含 `BlackHole`。

- [x] **Step 3: 运行两组定向测试，确认它们因缺少新行为而失败**

Run:

```powershell
cd frontend
pnpm exec tsx --test tests/lib/i10a-frontend-contract.test.ts
cargo test --manifest-path src-tauri/Cargo.toml --test i10a_windows_audio_red
```

Expected: 新增断言失败；原有断言继续运行。

### Task 2: 修复 Windows WASAPI 启动判断

**Files:**
- Modify: `frontend/src-tauri/src/audio/windows_loopback.rs`

- [x] **Step 1: 在 WASAPI 真正启动后立即确认成功**

在 `audio_client.Start()`、QPC 起始时间记录和格式记录成功后执行：

```rust
if let Some(sender) = startup_sender.take() {
    let _ = sender.send(Ok(()));
}
```

- [x] **Step 2: 移除首批数据对启动结果的控制**

从 `capture_loop` 参数和循环中删除 `startup_sender`，删除两条“回调超过两秒就返回错误”的路径。保留真实 WASAPI API 错误、线程错误、设备断开错误和音频数据统计。

- [x] **Step 3: 运行 Rust 定向测试**

Run:

```powershell
cargo test --manifest-path frontend/src-tauri/Cargo.toml --test i10a_windows_audio_red
```

Expected: Windows 音频契约测试全部通过。

### Task 3: 优化静默状态和错误引导

**Files:**
- Modify: `frontend/src/components/RecordingStatusBar.tsx`
- Modify: `frontend/src/components/RecordingControls.tsx`
- Modify: `frontend/src/app/page.tsx`
- Modify: `frontend/src/i18n/locales/en/recording.json`
- Modify: `frontend/src/i18n/locales/zh-CN/recording.json`

- [x] **Step 1: 显示可理解的等待状态**

系统音频流已启用但 `callback_count === 0` 时显示：

```tsx
{t('status.systemRoute')} {systemRoute.callback_count === 0
  ? t('status.waitingForSystemAudio')
  : `● ${((systemRoute.rms_level ?? 0) * 100).toFixed(0)}%`}
```

等待状态不使用红色，也不显示“音频错误”。

- [x] **Step 2: 给真实设备错误增加设置入口**

`RecordingControls` 接收 `onOpenDeviceSettings`，系统音频启动失败时显示主按钮：

```tsx
<button onClick={onOpenDeviceSettings}>
  {t('actions.openAudioDeviceSettings')}
</button>
```

首页将该回调连接到已有的 `showModal('deviceSettings')`。

- [x] **Step 3: 修正文案**

中英文新增 `actions.openAudioDeviceSettings`、`status.waitingForSystemAudio`，并将旧系统音频错误说明改为选择当前播放设备的明确指引，删除 BlackHole 和 macOS 权限内容。

- [x] **Step 4: 运行前端定向测试和国际化测试**

Run:

```powershell
cd frontend
pnpm exec tsx --test tests/lib/i10a-frontend-contract.test.ts
pnpm test:i18n
```

Expected: 所有测试通过，中英文键一致。

### Task 4: 全量验证、编译和自审

**Files:**
- Review all modified files

- [x] **Step 1: 运行格式化和静态检查**

Run:

```powershell
cargo fmt --manifest-path frontend/src-tauri/Cargo.toml -- --check
cd frontend
pnpm lint
```

Expected: 退出码为 0。

- [x] **Step 2: 运行前端生产构建**

Run:

```powershell
cd frontend
pnpm build
```

Expected: Next.js production build 完成，退出码为 0。

- [x] **Step 3: 运行 Rust 编译和相关测试**

Run:

```powershell
cargo test --manifest-path frontend/src-tauri/Cargo.toml --test i10a_windows_audio_red
cargo check --manifest-path frontend/src-tauri/Cargo.toml --no-default-features --features platform-default
```

Expected: 测试和编译退出码均为 0。

- [x] **Step 4: 自己进行五项代码审计**

检查正确性、可读性、架构、安全和性能；核对测试确实覆盖原始两秒误判；发现问题后先修复再重新运行验证。

- [x] **Step 5: 说明版本库限制**

当前交接目录没有 `.git`，因此不执行提交，也不能提供 Git diff；使用修改文件清单、内容检查和完整验证命令作为审计证据。

### Task 5: 重启桌面端并隔离验收系统音频

**Files:**
- Verify: `target/release/meetily.exe`
- Verify: `%APPDATA%/com.meetily.ai/recording_preferences.json`

- [x] **Step 1: 停止旧进程并启动本次编译产物**

核对运行文件为 `target/release/meetily.exe`，停止旧进程后重新启动，确认新窗口正常显示。

- [x] **Step 2: 使用“仅系统音频”排除麦克风串音**

在设置中切换到“仅系统音频”，重启应用后确认录音状态明确显示“仅系统音频”，不启动麦克风通道。

- [x] **Step 3: 验证静默启动不会误报错误**

开始录音并保持输出设备静默超过两秒，界面显示“等待电脑声音”，不再显示“系统音频不可用”。

- [x] **Step 4: 播放已知 WAV 并核对真实采集证据**

播放仓库内 `backend/whisper.cpp/samples/jfk.wav`。验收证据包括：系统音频状态从等待切换为活动、出现原生 QPC 时间、仅系统音频模式下实时转写出测试音频内容。

- [x] **Step 5: 停止测试并恢复用户原设置**

确认录音成功停止并保存；将录音模式恢复为“麦克风 + 系统音频”，再次读取持久化配置确认恢复成功。
