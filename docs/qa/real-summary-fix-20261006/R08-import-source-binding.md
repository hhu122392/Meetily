# 导入录音的转写来源验收（2026-10-07）

本项仅通过“历史导入录音的来源绑定”验收。真实长音频的摘要准确性仍未通过，其他后端修改不包含在本次提交中。

## 原因与修复

音频导入把实际引擎写在 `metadata.json` 的 `import_contract.provider`。摘要来源读取器以前只读顶层 `transcription_provider`，读不到就回退到当前全局引擎。因此切换默认引擎会错误标记以前的会议。

共享读取器增加一次嵌套字段回退。顶层历史/录音字段仍优先；MOSS 激活选择、原文、时间和来源摘要值保持原逻辑。没有新增调用、依赖或数据迁移。

## 实际证据

- 用真实的 28 分 27 秒 SenseVoice 导入会议 `meeting-d0949717-c809-4ca2-b107-aa157d88e68f`；当前数据库默认引擎是 `localWhisper`。
- 修复前，通过 Windows 客户端生成 `gen_4a749384c0094570bfb1ab468005c4fe`，历史错误记录为 `whisper / legacy_whisper_…`。
- 修复后，使用完整 release 0726 的 Windows 客户端生成 `gen_86af60520b0e4aa0aab3c863e5367e8b`，历史正确记录为 `sensevoice / legacy_sensevoice_…`，来源摘要值仍为 `17ea20fb50a3753fadf86e66211fade2fad004a39d0e47c47ab49909e61adf26`。
- 本次程序 SHA256：`572db72bf5d3f3eeba8d09ebfe90b28670646c92d8585ef4d9063452b0f67495`。已核对打包文件、编译产物和 21 份相关源码；前端和 helper 实际重建。
- 原生生成取得来源后通过桌面停止。新历史为 `cancelled`，原摘要的数据库序列化 SHA256 仍为 `6b8dab45b808ac69db1a157bbdc5408afb425a6c5f34c336a2f880cd4d23ebca`；原始音频、转写和 metadata 文件的 SHA256 全部相同。
- 新增公共读取 API 的回归检查：修复前实际失败（Whisper 与 SenseVoice 不符），修复后 6 个相关测试通过；覆盖默认引擎再次改变、顶层字段优先和缺失字段回退。

本机证据目录：`_ux_audit/真实长音频摘要修复验收-20261006/R08u/`，包含 `provider-native-after.json`、`provider-native-acceptance.json`、`provider-before.log`、`product-source_repository-tests.log`、`build-runtime-verification.json` 和原生截图 001–012。修复前原生证据在 `R08t/provider-native-reproduction.json`。

## 自行 review

只改共享来源读取处的一行和一个必要回归测试。该行使用已有 JSON/Option API，与当前 HEAD 的来源类型接口兼容。保留顶层字段优先顺序与原有缺失数据的兼容回退，不修改任何历史行或音频文本。来源标记通过不代表口语识别、翻译或摘要语义通过。
