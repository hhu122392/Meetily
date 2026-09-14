# Meetily MOSS v3 P6 发布与验收手册

这份手册只定义可重复执行的检查，不代表 P6 已通过。当前在 P3、P4、P5 尚未合并且没有真人批准的完整真值稿时，P6 必须保持 `NOT RUN`，L2 发布必须保持 `BLOCKED`。

以下命令都从仓库根目录执行。工具退出码统一为：`0=PASS`、`1=FAIL`、`2=BLOCKED 或 NOT RUN`。模板文件不能直接当证据；必须复制、替换全部 `__REQUIRED_*`、把 `template_only` 改为 `false` 后再执行。

## 一、现在就能执行的并行检查

先运行 P6 工具测试和构建接线检查：

```powershell
python -m unittest discover -s scripts/qa -p 'test_moss_v3_p6*.py' -v
python scripts/qa/moss_v3_p6_release.py build-wiring `
  --repo . `
  --output target/moss-v3-p6-evidence/build-wiring.json
```

构建接线报告只有在 Tauri 同时声明 `llama-helper`、`moss-helper`，本地准备脚本和五条构建工作流都用 lockfile 构建并复制两个 sidecar，正式 release 驱动确实调用已审计的 `build.yml` 并强制签名，且 Windows 发布工作流选择 Vulkan 时才会 `PASS`。这只证明构建脚本接线完整，不证明发布包已实际构建或签名有效。

用 P1 冻结锁复核当前 Q8 模型、CPU/Vulkan 运行库和许可证：

```powershell
python scripts/qa/moss_v3_p6_release.py assets `
  --repo . `
  --data-root 'D:\MeetilyData' `
  --deployment-root 'D:\MeetilyData' `
  --system-drive 'C:' `
  --model 'D:\MeetilyData\staging\moss-p1\MOSS-Transcribe-Diarize-Q8_0.gguf' `
  --model-license 'D:\MeetilyData\staging\moss-p1\LICENSE-MOSS-Transcribe-Diarize-Apache-2.0.txt' `
  --runtime 'D:\MeetilyData\staging\v0\runtime\transcribe-native-windows-x86_64-cpu-vulkan' `
  --runtime-license 'D:\MeetilyData\staging\v0\source\transcribe.cpp-v0.2.2\LICENSE' `
  --runtime-third-party-license 'D:\MeetilyData\staging\v0\source\transcribe.cpp-v0.2.2\THIRD-PARTY-LICENSES.md' `
  --output target/moss-v3-p6-evidence/asset-integrity.json
```

该命令检查文件名、字节数、SHA-256、运行库完整文件集、CPU/Vulkan contract、模型许可证、运行库许可证、ggml 和 miniz 许可证，并拒绝软链接或目录穿越。它会固定输出 `accuracy_status=NOT_SCORABLE_UNTIL_HUMAN_APPROVED_FULL_REFERENCE`；资产哈希通过不能替代准确率验收。

## 二、P3-P5 合并后的发布输入

最终发布 HEAD 必须是干净工作区。P0 到 P5 每阶段各提供一个规范化 gate JSON，格式见 `docs/release/moss-v3-stage-gate.template.json`。每个 gate 必须满足：

- `stage` 精确等于 `MOSS_V3_P0` 到 `MOSS_V3_P5`；
- `status=PASS`；
- `source_commit` 是完整 40 位提交号，并且是发布 HEAD 的祖先；
- `files` 至少一项，每项的相对路径、字节数和 SHA-256 与现场文件一致。

发布依赖不是“几条分支最后都能在 HEAD 找到”就算完成。验收工具和最终审计会强制检查 `P2 source_commit → P3 source_commit → P4 source_commit → P5 source_commit → P6 发布 HEAD` 的逐级 Git 祖先关系。当前 P6 并行工具提交只能先保存；P5 集成 HEAD 出现后，必须把该提交叠加到 P5 之上，再从新的发布 HEAD 重跑所有带 `source_commit` 的报告。

只写一个 `PASS` 字段不算通过。工具会重新检查 Git 祖先关系和每个证据文件的哈希。

## 三、Windows Vulkan 发布清单

复制 `docs/release/moss-v3-p6-release-spec.template.json`，填写发布版本、发布 HEAD、签名安装包目录、实际安装目录、D 盘 MOSS manifest 专属目录、迁移文件、发布说明和 Authenticode 报告。`package`、`application`、`moss_data` 三类根目录中的每个文件都必须有一条 `files` 记录；漏掉任意 DLL、资源、许可证或安装包旁车文件都会失败。不同角色不能指向同一个文件。

证据 manifest 是最终审计之后生成的下游文件，不写进前置 release manifest，避免 release manifest 和 evidence manifest 互相引用形成无法闭合的循环哈希。

```powershell
python scripts/qa/moss_v3_p6_release.py package `
  --repo . `
  --spec target/moss-v3-p6-evidence/release-spec.json `
  --output target/moss-v3-p6-evidence/release-manifest.json
```

`PASS` 至少表示：发布提交与 HEAD 相同、工作区干净、目标是 `windows-x86_64-vulkan`、所有必需角色存在、三类发布根目录没有未登记文件、安装目录和安装包目录没有 GGUF 或与 MOSS 模型/运行库相同的哈希、MOSS 文件只在 `D:\MeetilyData` 下的专属目录。

## 四、安装、升级、卸载和版本回滚

只在隔离的 Windows 测试账号或可还原虚拟机中执行安装器。先复制并填写 `docs/release/moss-v3-p6-snapshot.template.json`。测试资料必须包含旧会议、人工编辑、录音、数据库、设置、用户模板，以及已安装的 Whisper、Parakeet、Qwen、Gemma 模型；不能拿空目录代替保护对象。

七个检查点必须一个不少，使用同一份 snapshot config 和同一个发布 HEAD；工具会拒绝复用五个检查点冒充七个检查点：

1. `before_install`：没有安装应用，但已放入要保护的旧数据；
2. `after_install`：全新安装候选发布件后；
3. `before_upgrade`：旧稳定版和旧数据已就绪，尚未升级；
4. `after_upgrade`：从旧稳定版升级到候选发布件后；
5. `after_uninstall`：从 `after_upgrade` 状态卸载候选发布件后；
6. `before_rollback`：恢复 `after_upgrade` 的独立快照，再次记录回滚前状态；
7. `after_rollback`：安装批准的旧稳定版完成版本回滚后。

每次操作前先完全退出 Meetily、`moss-helper`、`llama-helper` 和 ffmpeg，让 SQLite WAL 落盘，再执行：

```powershell
python scripts/qa/moss_v3_p6_lifecycle.py snapshot `
  --repo . `
  --config target/moss-v3-p6-evidence/snapshot-config.json `
  --checkpoint before_install `
  --output target/moss-v3-p6-evidence/snapshots/before_install.json
```

其余六次只替换 `--checkpoint` 和输出文件名。然后复制 `docs/release/moss-v3-p6-lifecycle.template.json`，改成可执行 spec，并运行：

```powershell
python scripts/qa/moss_v3_p6_lifecycle.py verify `
  --repo . `
  --spec target/moss-v3-p6-evidence/lifecycle.json `
  --release-manifest target/moss-v3-p6-evidence/release-manifest.json `
  --snapshot before_install=target/moss-v3-p6-evidence/snapshots/before_install.json `
  --snapshot after_install=target/moss-v3-p6-evidence/snapshots/after_install.json `
  --snapshot before_upgrade=target/moss-v3-p6-evidence/snapshots/before_upgrade.json `
  --snapshot after_upgrade=target/moss-v3-p6-evidence/snapshots/after_upgrade.json `
  --snapshot after_uninstall=target/moss-v3-p6-evidence/snapshots/after_uninstall.json `
  --snapshot before_rollback=target/moss-v3-p6-evidence/snapshots/before_rollback.json `
  --snapshot after_rollback=target/moss-v3-p6-evidence/snapshots/after_rollback.json `
  --output target/moss-v3-p6-evidence/lifecycle-report.json
```

SQLite 使用同一个只读事务和 `PRAGMA integrity_check`。`tables="*"` 会检查全部用户表。新增表、新增尾部列和新增行可以通过；删表、删除旧行、改写旧值、改动旧列定义都会失败。所有 protected 根都必须标为 required；录音、模板以及 Whisper、Parakeet、Qwen、Gemma 测试根必须至少各有一个保护样本，数据库必须至少有一行，空目录不能作为“未丢数据”的证据。报告只保存结构和哈希，不保存会议标题、转录正文或设置值。

默认卸载策略是保留 D 盘 MOSS 数据。应用安装目录必须被删除，但 Whisper、Parakeet、Qwen、Gemma、录音、数据库、设置和模板必须保持。MOSS 专属清理另做测试，并且只能删除 manifest 中的路径。

## 五、离线和 28 项整链路验收

先在允许联网的相同测试机上选定三个独立、明确允许探测的公网 IP 和 TCP 端口，建立可达对照：

```powershell
python scripts/qa/moss_v3_p6_network_probe.py `
  --repo . `
  --mode reachable `
  --endpoint '__REQUIRED_PUBLIC_IP_1__:__REQUIRED_PORT_1__' `
  --endpoint '__REQUIRED_PUBLIC_IP_2__:__REQUIRED_PORT_2__' `
  --endpoint '__REQUIRED_PUBLIC_IP_3__:__REQUIRED_PORT_3__' `
  --output '__REQUIRED_SCENARIO_WORKDIR__\results\network-reachable-control.json'
```

随后用测试机的系统级网络隔离措施阻断出站网络。`moss_v3_p6_network_probe.py --mode blocked` 只有在三个直连 IP 全部失败时才通过；它不负责改防火墙。验收配置中的 `network-blocked-before` 和 `network-blocked-after` 会在离线主链路前后各复核一次。验收器会重新解析一个联网对照和两个阻断报告，要求三份报告都绑定同一发布 HEAD、同一组端点和同一个 contract SHA-256；只放三个同名文件或换一组不可达端点不能通过。

复制 `docs/release/moss-v3-p6-acceptance.template.json`。28 个场景 ID 不能增删，所有命令都要换成真实发布件驱动脚本，证据路径必须相对于各场景工作目录。P0-P5 任一 gate 缺失、无效或不是 `PASS` 时，验收工具不会启动任何产品命令，只输出 `NOT RUN`。

```powershell
python scripts/qa/moss_v3_p6_acceptance.py `
  --repo . `
  --config target/moss-v3-p6-evidence/acceptance.json `
  --stage-gate P0=__REQUIRED_P0_GATE__ `
  --stage-gate P1=__REQUIRED_P1_GATE__ `
  --stage-gate P2=__REQUIRED_P2_GATE__ `
  --stage-gate P3=__REQUIRED_P3_GATE__ `
  --stage-gate P4=__REQUIRED_P4_GATE__ `
  --stage-gate P5=__REQUIRED_P5_GATE__ `
  --public-output target/moss-v3-p6-evidence/acceptance-public.json `
  --private-output target/moss-v3-p6-private/acceptance-private.json `
  --private-log-dir target/moss-v3-p6-private/logs
```

公开报告只保留可执行文件名和哈希、命令哈希、耗时、退出码、日志哈希及证据文件哈希。完整 argv、本地绝对路径和原始日志只进 private 目录。超时会杀掉该命令的进程树；超时、缺证据文件或任一非预期退出码都会使场景失败。

## 六、真人真值和硬指标

先复制 `docs/release/moss-v3-p6-human-reference-approval.template.json`。真实复核人必须对着冻结音频检查完整 737.728 秒参考稿，填写复核人、UTC 时间、参考稿哈希、音频哈希和固定声明。再复制 `docs/release/moss-v3-p6-metrics.template.json`，把参考稿和批准记录都放在 metrics 报告旁边，分别记录相对路径、字节数和 SHA-256。只有这两份文件的绑定全部成立后，`ground_truth.status` 才能写 `HUMAN_APPROVED_FULL_REFERENCE`。

同一报告必须绑定 P1 冻结的 737.728 秒与 3096.62 秒音频哈希，并提供：MOSS 原始 CER、同窗口 Whisper CER、修正后 CER、实际术语命中率、未说术语新增数、两种原始说话人错误率、修正后人员错误数、时间戳解析/单调/边界、737 RTF、3096.62 秒到尾、MOSS 残留进程数和 Qwen 启动顺序。数字本身不算原始证据；`measurement_evidence` 必须逐文件绑定 MOSS 原始稿、同窗口 Whisper 稿、修正稿、术语评分、说话人评分、时间戳评分、性能日志和进程顺序日志的字节数与 SHA-256。

缺真人完整真值时 metrics 状态只能是 `BLOCKED`，不能用模型转录、摘要、抽样几分钟或主观判断代替。所有比率、CER、RTF、时间和计数都做类型及合法范围检查；负数、用布尔值冒充 `0`、NaN 或 Infinity 都会失败。

## 七、最终门和证据闭合

先生成最终审计。下面每个输入都必须由同一个发布 HEAD 产生：

```powershell
python scripts/qa/moss_v3_p6_release.py final-audit `
  --repo . `
  --stage-gate P0=__REQUIRED_P0_GATE__ `
  --stage-gate P1=__REQUIRED_P1_GATE__ `
  --stage-gate P2=__REQUIRED_P2_GATE__ `
  --stage-gate P3=__REQUIRED_P3_GATE__ `
  --stage-gate P4=__REQUIRED_P4_GATE__ `
  --stage-gate P5=__REQUIRED_P5_GATE__ `
  --release-manifest target/moss-v3-p6-evidence/release-manifest.json `
  --asset-report target/moss-v3-p6-evidence/asset-integrity.json `
  --build-wiring-report target/moss-v3-p6-evidence/build-wiring.json `
  --lifecycle-report target/moss-v3-p6-evidence/lifecycle-report.json `
  --acceptance-report target/moss-v3-p6-evidence/acceptance-public.json `
  --metrics-report target/moss-v3-p6-evidence/metrics.json `
  --output '__REQUIRED_P6_EVIDENCE_DIRECTORY__\p6-final-audit.json'
```

最终审计还会逐文件交叉检查 release manifest 和冻结资产报告：Q8 模型、全部锁定运行库文件以及许可证的名称/哈希必须真的出现在 D 盘发布清单中。只验证 staging 资产、却在发布目录放入另一套文件，不能通过。

最后按计划建立固定证据目录，并确保至少包含 `TASKS.md`、`00-source-state.json`、`01-input-manifest.json`、`02-environment.json`、`03-run-results.json`、`04-test-results.json`、`05-data-integrity.json`、`06-self-review.md`。截图不能替代日志、数据库检查和哈希。

当且仅当 `p6-final-audit.json` 中 P6 与 L2 都是 `PASS`、没有开放 gate、工作区干净、P0-P5 和全部报告都通过时，才执行最终证据 manifest：

```powershell
python scripts/qa/moss_v3_p6_release.py evidence-manifest `
  --repo . `
  --evidence-dir '__REQUIRED_P6_EVIDENCE_DIRECTORY__' `
  --p6-status PASS `
  --final-audit '__REQUIRED_P6_EVIDENCE_DIRECTORY__\p6-final-audit.json'
```

最终发布可以写 `PASS` 的必要条件是：该命令退出码为 0，`07-evidence-manifest.json.manifest_status=PASS`，`p6_status=PASS`，并且发布时再次核对其中全部文件哈希。任何输入变化后都必须从受影响的上游报告重新执行，不能手改旧 JSON。
