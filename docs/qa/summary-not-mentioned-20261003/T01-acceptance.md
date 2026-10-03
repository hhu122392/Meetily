# T01 人员状态提交和保存验收

2026-10-03，实施起点 bca0a6977f15ae84d4bb88373a13c7e31d6b2160。复用已有 checkout，未新建 worktree。

默认草稿与人工调整均在开始录音前提交、校验。模板加载中、失败或旧请求晚返回不能提交另一模板的人员；无人员预设仍允许录音。后端无草稿时的 expected 含义保持原样。

检查：`pnpm exec tsx --test tests/lib/use-recording-meeting-setup.test.tsx tests/lib/meeting-context.test.ts tests/lib/recording-service-ipc-contract.test.ts`，18通过，0失败，退出0。修复前默认出席正确断言退出1。Next 和 native 标准优化 Release 构建退出0。可执行文件0.4.3，SHA256 `6F8E97FB8BE47F82332EFB7A1A8E93E629185A4E87370045649C15F5EE968F1D`。

使用电脑能力操作真实桌面程序并核对 completed metadata、录音会议ID和模板来源，4场结果如下。

| 案例 | 会议ID | 已保存人员状态 | metadata SHA256 |
| --- | --- | --- | --- |
| P01 | meeting-9b0fb07b-40f0-4631-83fa-5abfccab6e46 | attending, attending, attending | 76410ebe4f7ec723b3420f4dce456965ba5819776c957a361f6f97c3d6393dd2 |
| P02 | meeting-1bed57b2-bba7-4530-9b8a-aa66093ac7ff | attending, attending, absent | e9e8f4296505d921ee1aa6c031acd360c7bf3adbf64baa3fcdba10cd46eb84da |
| P03 | meeting-7ac8f8b7-64c1-4291-abdf-e1006c50e945 | expected, expected, expected | 8ce4ede2ea0af58ecdeda1b32c3ecddabc80ce8a278453d8a8a1a6f2f6b58a18 |
| P05 | meeting-62b5a703-a2c6-47f0-9272-df89f2e83cf5 |  | b28e6af830940e38688cc6d3af50da8cec2bb74ea6377dec871259cd9ed955c1 |

P04 桌面执行全员缺席→恢复默认，3人出席；切标准模板后人员为空。乱序请求通过实际 hook 的延迟返回检查，最终模板、人员ID、哈希匹配。P05 非法草稿由 hook 检查验证，不宣称实际 UI 可输入非法哈希。

Review：生产改动4个文件；isCustomized仅供恢复按钮展示；旧会议 metadata 和模板原文件逐字节／哈希不变；18项断言实际执行业务 hook，未用业务 mock 替换 hook。未解决必改问题0。通过。

本地证据：`_ux_audit/会议摘要未提及修复验收-20261003/T01`，包括 hook-before.log、hook-after.log、frontend-build-v043.log、rust-build-v043.log、source-manifest.json、review-boundaries.json、Pxx截图与数据。静音录音仅验证人员保存，不算语音转写或摘要生成通过；后续任务单独验收。
