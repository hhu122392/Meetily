# Meetily 阶段 5A-3：官方历史版本升级、语义数据保留与受控回滚审计

## 1. 审计结论

- 执行日期：2026-08-23 至 2026-08-24；
- 技术子项：官方资产来源、冻结历史迁移源、脱敏夹具、实际产品身份升级、数据语义保留、受控回滚、全量回归通过；
- 阻断子项：直接静默降级不安全；官方 v0.3.0 运行时迁移未得到可验证证据；未使用经授权真实历史数据；0.4.1 审计候选未签名；
- 最终发布判定：**FAIL／NO-GO**。

本审计把“测试已完成”与“产品可以发布”分开。最终 Windows Sandbox 矩阵已运行到完整回滚和语义比较，但报告仍是 `passed=false`，原因是 `directDowngradeProtectionEffective=false`。这是有意的审计结果，不是未完成测试。

## 2. 范围和边界

### 2.1 纳入范围

- GitHub 官方 v0.3.0 Release 元数据、NSIS 资产、`latest.json`、脱离式更新签名和 tag 提交；
- 官方 v0.3.0 安装后 EXE 身份、签名、注册表、安装目录和启动存活；
- v0.3.0 和当前源码的 10 个 SQLx 迁移文件 blob 一致性；
- 无 Secret 的中英文脱敏会议、转录、摘要、备注、录音、设置和通知夹具；
- 实际产品身份 `meetily` / `com.meetily.ai` 的 0.4.1 未签名审计候选；
- 断网 Windows Sandbox 中的官方 0.3.0 → 候选 0.4.1 升级、损坏包阻断、降级观测、卸载保留和受控回滚；
- 升级前、升级后、回滚后的 SQLite schema／全行语义对比与非 DB 文件哈希对比；
- Lint、Node、i18n、TypeScript、Next、Rust、Doc-test 和 1000 模板性能预算。

### 2.2 明确不在范围内

- 未读取、复制或修改本机真实 Meetily 用户数据；
- 未安装或覆盖用户已安装的 `D:\桌面\开源版\meetily`；
- 未将 0.4.1 审计候选复制到 `target/release`；
- 未声明官方 v0.3.0 二进制已对 seed DB 执行迁移；
- 未执行生产代码签名、SmartScreen 声誉或发布渠道上传；
- 未关闭阶段 5 的多真机、60 分钟录音、真实 Provider、读屏和治理签字门禁。

## 3. 官方 v0.3.0 来源审计

### 3.1 Git 与 GitHub Release

| 项目 | 结果 |
|---|---|
| 标签 | `v0.3.0` |
| tag 类型 | 轻量 tag，object type = `commit` |
| tag 提交 | `91b0c0985932d0797e249033601afa14f22ee3d3` |
| GitHub Release ID | `292451374` |
| 发布时间 | `2026-03-03T12:30:54Z` |
| Release URL | `https://github.com/Zackriya-Solutions/meetily/releases/tag/v0.3.0` |
| tag 签名 | 不适用；轻量 tag 没有可验证的 tag object 签名 |

GPG 在当前环境不可用，但即使可用，轻量 tag 也没有可验证的 annotated-tag 签名。本报告依据 GitHub Release API digest、资产 Authenticode、更新签名和 tag commit 做多证据来源审计，并保留 tag 层的限制。

### 3.2 官方资产

| 资产 | 字节 | SHA-256 | 签名／验证 |
|---|---:|---|---|
| `meetily_0.3.0_x64-setup.exe` | 43,301,928 | `900C10A4EA05D991A06AB670886DE45DE08904903298A862A94FDC66F25420D9` | Authenticode `Valid` |
| v0.3.0 安装后 `meetily.exe` | 64,193,664 | `0C1A60560490CF78BC9C14303A1598DB0F05DC887A7E8EB7BD00B7EC04408045` | Authenticode `Valid` |
| 脱离式 updater 签名 | 416 | `F14436E01001E8CCD43AFEAB00FBFBCACC280006AD2A8F7A889B921591F13A34` | Ed25519/minisign PASS |
| `latest.json` | 3,377 | `4C05F28D90609AB98690219C1F3CCE63A0EE00E0AB2A8E0830F7712E2297DFC0` | 资产引用和签名关系通过 |

官方安装包签名主体为 Zackriya Solutions Private Limited，证书 thumbprint 以 `047286` 开头。GitHub API digest 与本地下载哈希一致。更新签名使用配置的 key ID `ECA631D78797C82A` 同时验证主注释和 trusted comment。

## 4. 离线沙箱前提

- Windows Sandbox `Networking=Disable`；
- vGPU 禁用；
- 内存 6,144 MiB；
- 除输出目录外，资产、夹具和脚本均以只读目录映射；
- 未映射宿主 AppData、Music 或用户已安装目录；
- WebView2 使用 Microsoft 官方 x64 Evergreen Standalone Installer，212,949,712 字节，SHA-256 `82B2D8A7013E0C0EA15D48FF4742EE3778BA16BD8B7B4A47876645B3E48D4016`，Authenticode `Valid`；
- 断网安装后 WebView2 Runtime 版本为 151.0.4129.101。

## 5. 脱敏历史夹具

### 5.1 夹具性质

`fixtureKind` 为 `synthetic-redacted-frozen-v030-source-migrated-and-enriched`。它不是用户真实备份，也不声明由 v0.3.0 官方二进制迁移。

| 数据类别 | 数量／状态 |
|---|---|
| 会议 | 3 |
| 转录 | 5 |
| 摘要处理结果 | 2 |
| 转录分块 | 2 |
| 会议备注 | 2 |
| 应用设置 | 1 |
| 转录设置 | 1 |
| `_sqlx_migrations` | 10 |
| WAV 录音 | 2，确定性生成 |
| 文本语言 | 英文、简体中文、中英混排 |
| API Key | 全部为 `null` |
| 真实用户数据 | 无 |
| Secret | 无 |

数据库大小 90,112 字节，SHA-256 为 `C27B000DCEF5B496004DDD7B73E32BBB1F703952B029B27182BF46C97C312F3A`。夹具还包含 onboarding、preferences、recording preferences、notifications 和 `phase5a3-preservation-sentinel.txt`。

### 5.2 迁移来源

- v0.3.0 的 10 个迁移 Git blob 与当前候选一致；
- 夹具生成器逐个执行冻结 SQL 源；
- `_sqlx_migrations` 使用 SQLx 0.8.6 兼容的 SHA-384 checksum；
- `migrationExecution.mode=frozen-source-sqlx-compatible`；
- `migrationExecution.runtimeMigrationClaimed=false`。

### 5.3 官方运行时迁移探针

使用仅含初始 schema 的 seed DB 对官方 v0.3.0 做了多轮沙箱探针。官方应用能安装并保持运行，数据目录发现为 `%APPDATA%\com.meetily.ai`，但 seed DB 的字节数、SHA-256 和时间戳均不变，也没有 WAL/SHM。

CDP 探针尝试调用 `get_database_directory` 和 `check_first_launch`，但 WebView2 远程调试端口不可达，结果为 `Unable to connect to the remote server`。该探针记为失败，不作为任何通过证据。

结论：`P5-UPG-RUNTIME-MIGRATION-001` 继续 OPEN。

## 6. 0.4.1 审计候选

### 6.1 产品身份

| 字段 | 值 |
|---|---|
| productName | `meetily` |
| version | `0.4.1` |
| identifier | `com.meetily.ai` |
| NSIS installMode | `currentUser` |
| 候选用途 | Windows Sandbox 审计，不允许生产安装 |

### 6.2 构建输入与命令

- 依赖恢复：`pnpm install --offline --frozen-lockfile`，882 resolved，882 reused，0 downloaded，Exit 0；
- 构建：`pnpm exec tauri build --bundles nsis --config src-tauri/tauri.phase5a3.conf.json --no-sign --ci`；
- Next：13/13 静态页；
- Rust release 构建：143m 07s；
- FFmpeg：99,264,000 字节，SHA-256 `5AF82A0D4FE2B9EAE211B967332EA97EDFC51C6B328CA35B827E73EAC560DC0D`；
- llama-helper：3,773,440 字节，SHA-256 `EE4E303B64603DFF8369644EDF3792ADD1948605C62F3BEC66A2549757EF97EC`。

### 6.3 候选产物

| 产物 | 字节 | SHA-256 | 签名 |
|---|---:|---|---|
| 独立 `meetily-0.4.1.exe` | 65,776,128 | `4A1B762599ED79108DCE6FA56EF41162E11AF91723B34CC688E3F32567C3555A` | NotSigned |
| `meetily_0.4.1_x64-setup.exe` | 43,986,293 | `AD6D1B4B702873CBECBAD9CADD4F7166BC2C505DE70ACFCA226E1F8C46EF79EC` | NotSigned |
| NSIS 安装负载 EXE | 65,776,128 | `4B35A0398F49A0CC514791888D8857A2D9F576794ECC8120B401D4FDC419DFBD` | NotSigned |

### 6.4 独立 EXE 与 NSIS 负载差异

两份 EXE 只有 3 个连续字节不同，偏移为 `0x330042A` 至 `0x330042C`：

| 产物 | Hex | ASCII | 上下文 |
|---|---|---|---|
| 独立 EXE | `55 4E 4B` | `UNK` | `__TAURI_BUNDLE_TYPE_VAR_UNK` |
| NSIS 负载 | `4E 53 53` | `NSS` | `__TAURI_BUNDLE_TYPE_VAR_NSS` |

`Cargo.lock` 锁定 `tauri-utils 2.9.1`，checksum 为 `d57200389a2f82b4b0a40ae29ca19b6978116e8f4d4e974c3234ce40c0ffbdec`。其 `src/platform.rs` 345-360 行说明 `UNK` 是供 build patching 的初始值，`NSS` 映射 `BundleType::Nsis`。因此差异是打包器标记修补，不是产品代码二次编译。

## 7. Windows Sandbox 升级矩阵

### 7.1 最终流程

1. 校验官方安装包、WebView2、候选 EXE/NSIS、夹具清单和 DB 哈希；
2. 校验官方与 Microsoft Authenticode，同时确认候选 `NotSigned`；
3. 断网安装 WebView2；
4. 静默安装官方 0.3.0，校验注册版本、安装后 EXE 哈希、版本与签名；
5. 恢复脱敏夹具，启动官方 0.3.0 并保持 15 秒；
6. 保存升级前快照；
7. 复制候选安装包并翻转一个字节，在执行前因哈希不匹配阻断；
8. 安装原始候选，校验 0.4.1 注册版本与 NSIS 负载哈希；
9. 启动候选并保持 15 秒，保存升级后快照；
10. 直接运行官方旧版安装包，记录降级行为和安装目录残留；
11. 按安装器所有权卸载降级后的官方版本，保留 AppData；
12. 重新安装候选，校验候选身份；
13. 用候选自身卸载器删除候选资源，轮询程序目录，确认 AppData 保留；
14. 恢复升级前快照；
15. 重新安装官方 0.3.0，校验哈希、版本、签名和 15 秒启动；
16. 保存回滚后快照，执行受保护文件比较；
17. 卸载沙箱内最终官方版本，写入审计报告并关闭 Sandbox。

### 7.2 五次迭代审计记录

| 轮次 | 硬失败／发现 | 处置 |
|---|---|---|
| Attempt 1 | 安装后 EXE 哈希与独立 EXE 不同 | 未放宽哈希；冻结 NSIS 负载并做字节分析 |
| Attempt 2 | 升级、启动、回滚和语义通过，但卸载后 2 秒程序目录仍存在 | 不按时序豁免；改为最长 30 秒轮询并记录残留 |
| Attempt 3 | 30 秒后仍留下 12 个 `templates/en` / `templates/zh-CN` 文件 | 定性为直接降级的跨版本所有权污染 |
| Attempt 4 | 直接在降级后 0.3 上重装候选并卸载，仍留下 0.3 的 6 个根模板 | 证明两个安装器资源所有权必须按顺序清理 |
| Attempt 5 | 先用 0.3 卸载器清理旧版资源，再重装／卸载 0.4.1 | 受控回滚通过；直接降级阻断仍失败 |

前四次失败报告全部保留，没有只保留最后结果。

### 7.3 最终关键结果

| 断言 | 结果 |
|---|---|
| 冻结输入哈希 | PASS |
| 夹具合同与无 Secret | PASS |
| 官方安装包和 EXE 身份 | PASS |
| 离线 WebView2 | PASS |
| 损坏候选执行前阻断 | PASS |
| 候选原位升级 | PASS |
| 候选启动 | PASS |
| 升级后受保护文件 | PASS |
| 直接降级行为可确定观测 | PASS |
| 直接降级保护有效 | **FAIL** |
| 直接降级完成 | Exit 0，注册版本变为 0.3.0 |
| 直降后新版本地化模板残留 | 12 个 |
| 所有权顺序清理 | PASS |
| 候选卸载后注册表删除 | PASS |
| 候选卸载后程序目录删除 | PASS，1.015 秒 |
| 候选卸载保留 AppData | PASS |
| 官方 0.3.0 受控回滚 | PASS |
| 回滚后官方签名和 EXE 哈希 | PASS |
| 回滚后官方启动 | PASS |
| 最终报告 | `passed=false`，仅因直接降级保护失败 |

## 8. 语义等价审计

### 8.1 SQLite

升级前、升级后、回滚后 DB 均为：

- 文件 SHA-256：`C27B000DCEF5B496004DDD7B73E32BBB1F703952B029B27182BF46C97C312F3A`；
- `PRAGMA integrity_check`：`ok`；
- 语义 SHA-256：`A8C4DBE46E02DD532542F286C23A233443DDD31ABDD409921E315D613DBBF407`；
- `_sqlx_migrations`：10 行；
- 会议、转录、摘要、备注、分块、设置和 API Key 空值全部逐表逐行相等。

### 8.2 非 DB 文件

- 升级后：无缺失、无变更、无新增；
- 回滚后：无缺失、无变更、无新增；
- preferences、onboarding、recording preferences、哨兵、WAV、metadata、transcripts 全保留；
- notifications 比较忽略运行时管理的 permission bit，但强制比较用户控制的通知开关；
- 10 个语义断言全部为 `true`。

## 9. 全量回归与性能

| 门禁 | 结果 |
|---|---|
| ESLint | 0 error／0 warning |
| Node unit | 51/51 |
| i18n | 54/54 |
| TypeScript | Exit 0 |
| Next | 13/13 |
| Rust 首次全量 | 282 passed／1 failed／2 ignored；P95 682.8108 ms |
| 隔离性能复测 1 | P95 292.6903 ms，PASS |
| 隔离性能复测 2 | P95 270.2783 ms，PASS |
| 隔离性能复测 3 | P95 221.1673 ms，PASS |
| Rust 全量复测 | 283 passed／0 failed／2 ignored |
| Rust Doc-test | 1/1 |

首次失败发生在 Rust 测试与 Windows Sandbox/大量 I/O 并发时。审计不删除该失败，而是通过三轮隔离复测和一轮全量复测完成归因。500 ms 预算仍是硬门槛。

## 10. 分部分验收审计标准

| 编号 | 审计部分 | 强制验收标准 | 证据 | 结果 |
|---|---|---|---|---|
| 5A3-A | 历史来源 | API digest、Authenticode、安装后身份、tag commit、updater 签名一致 | provenance / updater signature JSON | PASS |
| 5A3-B | 离线环境 | 网络禁用，WebView2 官方包哈希与签名有效 | final upgrade report | PASS |
| 5A3-C | 夹具安全 | 无真实数据、无 Secret、API Key 全空，完整覆盖会议数据 | fixture manifest / semantic audit | PASS |
| 5A3-D | 官方运行时迁移 | 官方 v0.3.0 二进制对 seed DB 留下可验证迁移证据 | seed migration reports | **FAIL** |
| 5A3-E | 冻结源迁移 | 10/10 历史 blob 一致，SQLx checksum 与 schema 正确 | provenance / fixture manifest | PASS |
| 5A3-F | 候选可追溯 | 产品身份、构建命令、输入、候选和负载哈希固定 | candidate manifest | PASS（仅审计） |
| 5A3-G | 损坏包防护 | 哈希不匹配的包未执行 | corruption gate | PASS |
| 5A3-H | 官方→候选升级 | Exit 0、注册 0.4.1、负载哈希正确、存活 15 秒 | final upgrade report | PASS |
| 5A3-I | 数据语义 | DB integrity/schema/rows 与非 DB 文件在升级后等价 | semantic audit | PASS |
| 5A3-J | 直接降级保护 | 旧版不得覆盖新版，不得留下跨版本资源 | Attempt 3/5 | **FAIL** |
| 5A3-K | 受控回滚 | 按所有权清理、AppData 保留、快照恢复、官方重装与启动 | final upgrade report | PASS |
| 5A3-L | 回滚语义 | DB schema/rows 与非 DB 文件在回滚后等价 | semantic audit | PASS |
| 5A3-M | 二进制差异 | 差异可定位、可重复，无未解释产品代码漂移 | bundle marker audit | PASS |
| 5A3-N | 全量回归 | 前端、Rust、Doc-test 全通过，P95 < 500 ms | performance rerun audit | PASS |
| 5A3-O | 真实历史数据 | 经授权真实备份升级与回滚通过 | 未执行 | OPEN |
| 5A3-P | 代码签名 | EXE/NSIS 的 Authenticode、时间戳和证书链通过 | candidate manifest | OPEN |
| 5A3-Q | 正式产物保护 | 正式冻结 EXE/NSIS 哈希不变 | 本阶段末双哈希复验 | PASS |

## 11. 缺陷、风险和门禁

### 11.1 已关闭

- `P5-UPG-SCHEMA-001`：脱敏冻结源夹具升级／回滚语义通过；
- `P5-ROLLBACK-001`：受控回滚在脱敏夹具上通过；
- `P5-PKG-MARKER-001`：独立 EXE 和 NSIS 负载差异已定位为 Tauri 三字节标记修补；
- `P5-PERF-001`：三轮隔离性能复测和全量复测通过。

### 11.2 未关闭／阻断

- `P5-UPG-RUNTIME-MIGRATION-001`：官方 v0.3.0 未对 seed DB 留下可验证迁移证据；
- `P5-UPG-REALDATA-001`：没有经授权真实历史备份验收；
- `P5-DOWNGRADE-001`：旧版官方安装包可直接静默降级新版并留下 12 个新版模板；
- `P5-SIGN-001`：0.4.1 审计候选为 `NotSigned`；
- 多真机、多平台、60 分钟录音、真实 Provider、读屏、原生 Shell 和治理签字仍 OPEN。

## 12. 必须的后续处置

1. 发布程序不得支持或暗示用户可直接运行旧版安装包降级；
2. 必须将“按安装器所有权顺序清理 → 保留 AppData → 恢复备份 → 安装目标旧版”固化为受控回滚工具或运维 Runbook；
3. 必须为直接降级增加可实际执行的保护或检测，并在实际产品身份上复验；
4. 必须查明官方 v0.3.0 为何未对 seed DB 执行迁移，或提供可验证的真实 v0.3.0 历史备份与数据库版本证据；
5. 只能在获得明确授权和备份后使用真实历史数据；
6. 必须构建并验证带时间戳的生产 Authenticode 候选；
7. 完成阶段 5 其余真机、硬件、Provider、可访问性和治理门禁前，继续保持 NO-GO。

## 13. 证据索引

### 13.1 核心证据

- `docs/i18n/phase-5a3/candidate-v041-manifest.json`；
- `docs/i18n/audit/phase-5a3/official-v030-provenance-audit.json`；
- `docs/i18n/audit/phase-5a3/official-v030-updater-signature-audit.json`；
- `docs/i18n/audit/phase-5a3/official-v030-identity-probe.json`；
- `docs/i18n/audit/phase-5a3/historical-v030-enriched-fixture-manifest.json`；
- `docs/i18n/audit/phase-5a3/official-v030-to-v041-upgrade-audit.json`；
- `docs/i18n/audit/phase-5a3/upgrade-semantic-equivalence-audit.json`；
- `docs/i18n/audit/phase-5a3/candidate-executable-bundle-marker-audit.json`；
- `docs/i18n/audit/phase-5a3/rust-template-performance-rerun-audit.json`。

### 13.2 失败和调查证据

- `official-v030-seed-migration-webview-bootstrap-timeout.json`；
- `official-v030-seed-migration-liveness-only.json`；
- `official-v030-seed-migration-process-exit.json`；
- `official-v030-seed-migration-not-observed.json`；
- `official-v030-database-path-discovery.json`；
- `official-v030-cdp-database-probe.json`；
- `official-v030-to-v041-upgrade-attempt1-hash-mismatch.json`；
- `official-v030-to-v041-upgrade-attempt2-uninstall-timing.json`；
- `upgrade-semantic-equivalence-attempt2-audit.json`；
- `official-v030-to-v041-upgrade-attempt3-direct-downgrade-residuals.json`；
- `official-v030-to-v041-upgrade-attempt4-cross-version-residuals.json`。

### 13.3 可重放脚本与配置

- `docs/i18n/scripts/generate-phase5a3-historical-fixture.py`；
- `docs/i18n/scripts/enrich-phase5a3-v030-fixture.py`；
- `docs/i18n/scripts/bootstrap-phase5a3-v030-migrated-fixture.ps1`；
- `docs/i18n/scripts/audit-phase5a3-sandbox-upgrade.ps1`；
- `docs/i18n/scripts/audit-phase5a3-upgrade-semantics.py`；
- `docs/i18n/phase-5a3/sandbox-v030-to-v041-upgrade.wsb`；
- `frontend/src-tauri/tauri.phase5a3.conf.json`。

## 14. 正式产物保护

| 正式冻结产物 | SHA-256 | 结果 |
|---|---|---|
| `target/release/meetily.exe` | `1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823` | 未变 |
| `target/release/bundle/nsis/meetily_0.4.0_x64-setup.exe` | `C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434` | 未变 |

0.4.1 审计候选仅位于 `target-phase5a3-build/artifacts`。本阶段没有将任何候选 EXE 或安装包写入正式发布位置。
