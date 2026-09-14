# MOSS P4 与 P3 真实集成说明

父基线：P3 最终提交 `4ecbbad4fdc6b68e10309eb223af7e3e4b839522`

范围：P4 的 Meetily 前端、Tauri 命令适配、候选审核和相关测试。没有修改 P3 的 `frontend/src-tauri/migrations`，也没有处理 P5/P6 的模型安装、发布或部署工作。

## 已接通的事实

P4 不再使用“等待 P3”的合同模拟。`frontend/src-tauri/src/lib.rs` 已注册 10 个 MOSS 命令，`frontend/src-tauri/src/moss_review.rs` 直接调用 P3 的 `MossCandidateRepository`，启动和取消则连接 P2 的真实 `MossHelperManager`。

浏览器存储不是候选数据源。任务、候选片段、人员绑定、单片段覆盖、术语修正、激活快照和回退状态都从 SQLite 重建。关闭页面或重新启动应用后，前端通过 `api_moss_get_workspace` 重新读取权威状态。

内部开关 `moss_post_meeting_enhancement` 仍默认关闭，因为 P5 的模型和运行时交付没有包含在 P4。开关关闭不会改变原有 Whisper 重转录。

## Tauri 命令

所有请求都放在一个 `request` 字段下并使用 `camelCase`。写命令成功后返回最新 `MossWorkspace`，前端不在本地猜测写入结果。

| 命令 | 真实实现 |
|---|---|
| `api_moss_get_system_status` | 检查 Helper、固定运行时合同和模型文件哈希；运行 Helper probe；从 MOSS 存储路径所在挂载盘读取真实剩余磁盘空间；不返回路径 |
| `api_moss_get_workspace` | 在一个 SQLite 读事务中重建任务、现行转写、候选、参会人、绑定、修正和激活状态 |
| `api_moss_start_run` | 解析受控会议录音路径，预占真实 Helper，写入 P3 run，后台执行真实转录，再把结果写成 P3 候选 |
| `api_moss_cancel_run` | 用内存中的 `runId -> helper requestId` 映射调用真实 Helper 取消；终态由后台任务按 Helper 结果写入 P3 |
| `api_moss_save_speaker_binding` | 校验本次上下文中的人员后调用 P3 批量说话人绑定；`personId=null` 撤销当前绑定 |
| `api_moss_save_segment_override` | 调用 P3 单片段人员覆盖；覆盖优先于批量绑定 |
| `api_moss_set_correction_state` | 撤销 P3 最新修正，或新增一条可审计记录完成重应用 |
| `api_moss_update_candidate_segment` | 以 P3 片段覆盖记录保存人工候选文字，不修改现行转写 |
| `api_moss_activate_candidate` | 先校验候选 revision 和现行哈希，再调用 P3 单事务激活 |
| `api_moss_rollback_activation` | 校验 activation ID 和现行哈希，再调用 P3 单事务恢复激活前快照 |

前端命令名的唯一来源是 `frontend/src/features/moss/service.ts` 的 `MOSS_COMMANDS`。Tauri 注册表和前端测试逐项核对同一组名称。

## 使用的 P3 接口

P4 直接使用下面的 P3 仓库能力：

- `MossCandidateRepository::start_run`
- `MossCandidateRepository::complete_run_from_managed`
- `MossCandidateRepository::mark_run_failed`
- `MossCandidateRepository::mark_run_cancelled`
- `MossCandidateRepository::get_run`
- `MossCandidateRepository::current_transcript_sha256`
- `MossCandidateRepository::bind_speaker`
- `MossCandidateRepository::set_segment_override`
- `MossCandidateRepository::revoke_segment_override`
- `MossCandidateRepository::add_term_correction`
- `MossCandidateRepository::revert_latest_term_correction`
- `MossCandidateRepository::activate_candidate`
- `MossCandidateRepository::rollback_active_activation`

P3 没有提供“撤销当前说话人绑定”的公开方法。P4 的窄适配只更新 P3 已有 `moss_speaker_bindings.revoked_at` 字段，没有增加表、字段或迁移。后续若 P3 增加公开撤销方法，应把这条窄 SQL 替换为仓库调用。

## 一致性和冲突处理

候选 revision 由 P3 的追加式审计记录确定：初始候选为 revision 1；新增绑定、覆盖、修正以及对应撤销都会增加 revision。所有人工写命令在同一适配锁内先比较 `expectedCandidateRevision`，旧版本返回 `MOSS_CANDIDATE_STALE`。

只要某会议存在活动激活快照，绑定、覆盖、术语状态和候选文字编辑都会返回 `MOSS_CANDIDATE_CONFLICT`。激活还会比较 `expectedCurrentTranscriptSha256`；现行转写变化时返回 `MOSS_ACTIVATION_CONFLICT`。回退同样比较 activation ID 和当前已激活哈希，冲突时不覆盖人工内容。

单片段显示优先级固定为：

1. 人工片段文字覆盖；
2. 当前应用的明确别名修正；
3. MOSS 原始候选文字。

人员显示优先级固定为：

1. 单片段人员覆盖；
2. 匿名说话人标签的批量绑定；
3. 匿名状态。

## 姓名和术语修正边界

自动修正只读取会议上下文里明确登记的两类映射：

- `people[*].aliases -> display_name`，规则 ID 前缀为 `PERSON_ALIAS_`；
- `terms[*].aliases -> canonical`，规则 ID 前缀为 `TERM_ALIAS_`。

匹配只在 immutable `raw_text` 上收集。候选按“最长优先、互不重叠、确定性排序”选择，然后从右往左写入 P3 修正记录，替换后的文字不会成为下一条规则的命中来源。因此 A→B 不会继续触发 B→C。

ASCII 别名必须满足单词边界。跨 PERSON/TERM 的同一比较键若对应不同标准值，会被视为歧义并全部跳过。未登记的相似姓名不会自动新增，匿名说话人标签也不会被用来猜姓名。

## 状态、磁盘和错误安全

`availableDiskBytes` 来自 MOSS 运行时存储路径所在挂载盘的 `available_space()`。Helper probe 返回的设备内存不会写入这个字段。

系统状态和错误都不返回绝对路径。公开错误只有：

```json
{
  "code": "MOSS_CANDIDATE_STALE",
  "retryable": true,
  "debugId": "moss-safe-reference"
}
```

数据库文本、会议正文、姓名、术语正文、音频路径、堆栈和 Secret 不会进入错误响应。若前端与旧桌面程序组合导致命令缺失，前端把非结构化 Tauri 错误改写为 `MOSS_FRONTEND_INTEGRATION_UNAVAILABLE`，不显示原始文本。

## 当前验证范围

Rust 的 P4 SQLite/命令实现测试覆盖：

- 从 SQLite 重建 workspace，并关闭连接后重新打开数据库再次得到相同结果；
- 批量说话人绑定、单片段人员覆盖及其优先级；
- 人工候选编辑和旧 revision 冲突；
- 术语撤销与重应用；
- 激活后禁止编辑；
- 激活和回退返回一致 workspace；
- 重复启动由真实 P3 仓库拒绝；
- `runId -> helper requestId` 取消映射；
- 错误载荷脱敏；
- 正向别名、ASCII 边界、最长不重叠、链式负例；
- 姓名别名正例、未登记相似姓名负例、跨命名空间歧义负例；
- 磁盘字段来自传入存储目录所在挂载盘。

前端测试继续覆盖运行时响应校验、候选/现行对比、写入动作、功能开关、错误状态和无障碍属性。

## P4 之外的边界

P4 没有下载或安装固定 MOSS 模型和运行时，也没有修改其发布清单；这是 P5 范围。因此本阶段可以证明真实命令、真实 P3 持久化和真实 P2 Helper 调用路径已经接通，但不能在缺少已验哈希资产的机器上声称完成了真实模型端到端转录。

P4 没有处理 P6 发布、长时性能或安装升级验收。
