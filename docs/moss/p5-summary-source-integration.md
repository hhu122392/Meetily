# P5 摘要来源绑定与 P3/P4 集成说明

本文记录 P5 在 P4 集成基线 `0efe4fd82b8c6e448815bde189074cb42a8d44f5` 上的真实落地边界。它只说明代码、迁移和自动测试覆盖到哪里，不代表真人准确率、真实模型端到端或发布门禁通过。

## 已闭合的 P3 来源读取

- `api_process_transcript` 仍保留前端传入的 `text` 参数以兼容现有调用，但 Rust 会立即丢弃它。摘要正文只从原生 SQLite 重新构建。
- `summary/source_repository.rs` 在一个数据库事务里读取 P3 的 `moss_activation_snapshots` 和 `moss_activation_segments`。只有 `status = 'active'` 的激活快照能进入摘要，片段按 `segment_index` 读取。
- MOSS 候选、完成但未激活的 run、失败 run 和 `rolled_back` 快照都不在生产查询里。出现多个 active、空快照、坏时间或坏片段时会停止生成，不能静默改用别的版本掩盖数据损坏。
- 没有 active MOSS 快照时，读取当前 `transcripts` 作为 Whisper 兼容来源。因此 MOSS 失败不会占用摘要来源，也不会阻断原有 Whisper 摘要链。
- MOSS 来源使用真实 `activation_id`、`run_id` 和 `activated_at`。转录版本号和人员绑定版本号按该会议不可删除的 activation 快照数计算；再次激活会得到新的 activation 和递增版本。
- 摘要读取的是 P3 激活时冻结的片段和人员绑定，不读取后来可能变化的兼容 `transcripts` 行。未绑定说话人仍是 `S01/S02`，只有激活快照里同时存在人员 ID 和显示名时才显示真人姓名。
- P5 会重新计算转录正文哈希和人员绑定哈希，并走 `select_activated_summary_source` 的领域校验。哈希不匹配、会议不匹配或来源字段不完整都会停止生成。

## 可查询的生成历史

迁移 `20260830000000_add_summary_source_lineage.sql` 给 `summary_generation_history` 增加了以下可查询列：

| 类别 | 字段 |
| --- | --- |
| 来源 | `source_binding_schema_version`、`transcript_source` |
| 转录版本 | `transcript_version_id`、`transcript_version`、`transcript_activated_at` |
| MOSS | `moss_run_id` |
| 转录完整性 | `transcript_sha256` |
| 人员绑定 | `speaker_binding_snapshot_id`、`speaker_binding_version`、`speaker_binding_sha256` |
| 模板 | 原表已有的 `template_id`、`template_version`、`file_sha256`、`semantic_sha256` |

新生成会在创建 `summary_processes` 的同一数据库事务里写入完整来源字段。列表仓储和 Tauri 历史接口会从关系列真实读取这些值，不依赖 JSON 快照反查。旧历史行允许这些新增列为 `NULL`，这样升级不会伪造旧来源。关系表不保存转录正文、提示词、密钥或绝对路径。

`create_or_reset_process_with_generation_history` 仍以一次请求插入一个新的 generation，并把旧 pending generation 标成 `superseded`。前端还用同步 ref 合并 React 同一事件循环里的重复生成调用；这不把用户后来主动发起的第二次生成误当成同一次点击。

## 旧摘要过期标记

每次生成保存以下不可变绑定：

- 转录来源、版本 ID、版本号、MOSS run ID、激活时间和转录哈希；
- 人员绑定快照 ID、版本号和绑定哈希；
- 模板 ID、版本号、文件哈希和语义哈希。

`api_get_summary`、人工保存返回值和人工历史恢复返回值都会在 Rust 端重新读取当前 active 来源与当前真实模板，并附加 `summaryFreshness`。转录版本、正文、人员绑定或模板任一变化都会得到 `stale` 和对应原因。旧记录没有可信绑定或当前来源无法读取时返回固定的 `unavailable` 标记，原生错误细节只写日志，不直接暴露给前端。

摘要面板把 stale/unavailable 独立显示为“需检查”，不会覆盖事实校验状态。用户在页面里切换到不同模板时，前端会立即补充模板变化提示，保存后仍由 Rust 重新计算。人工保存会删除 WebView 自带的 `factValidation` 和 `summaryFreshness`，再按当前原生证据生成，避免浏览器伪造通过状态。

## 事实与编辑链路

- 负责人和时间只有在字段值与同一行动项出现在同一转录片段时才算有证据。追溯信息保存片段 ID、起止毫秒和片段 SHA-256，不额外复制正文。
- 自动生成缺少证据时保持 `会议未提及`；人工编辑可保留用户内容，但状态会变成 `needs_review`，中文明确显示“需检查”。
- 摘要轮询现在保留 Rust 返回的完整结果，避免丢失模板绑定、事实校验和 freshness。编辑时也保留这些原生字段。
- 编辑、保存、复制按钮、生成历史、人工修订历史和恢复入口没有被 P5 替换。恢复后会重新按当前转录做事实校验，并重新附加来源过期状态。

## 没有覆盖的 P3/P4 文件

P5 没有修改 P3 的 `20260829000000_add_moss_candidate_store.sql` 和 MOSS 激活事务，也没有修改 P4 的 MOSS 候选审阅页面。新增内容是独立的 P5 迁移、只读来源适配器、摘要仓储字段和摘要面板提示。P3/P4 的真实仓储与命令测试仍作为叠加后的回归门禁运行。

## 仍然阻断的真实模型证据

仓储测试能证明“失败 MOSS 不会成为 active，Whisper 仍可作为摘要输入”，并能证明 `qwen3.5:2b` 的 generation 可带完整来源字段写入和读取；这不是一次真实 MOSS + Whisper + Qwen 推理。

真实 Qwen 测试 `mc_r04_real_qwen_2b_summary_respects_verified_meeting_facts` 需要实际会议目录、Qwen 3.5 2B 模型目录、模板路径和证据输出路径。本机没有设置这四项输入，也没有在项目数据目录找到对应会议夹具和模型文件，因此该项是 **BLOCKED**，不能用领域测试、模拟输出或旧证据替代。

另外，当前 P4 基线的新装模型选择是按内存推荐：低于阈值时选择 `qwen3.5:2b`，达到阈值时选择 `qwen3.5:4b`。P5 没有改写这项既有策略；它会把实际选择的模型名原样写入历史。因而只能证明 2B 路径未被 P5 阻断，不能声称所有机器的新装默认都固定为 2B。
