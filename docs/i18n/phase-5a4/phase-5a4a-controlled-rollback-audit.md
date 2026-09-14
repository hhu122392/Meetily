# Meetily 阶段 5A-4A：受控回滚与安装资源所有权隔离严格审计报告

## 1. 审计结论

- 执行日期：2026-08-24；
- 实施基线：`4bdcb0767314e602b8ce06aa6ef97c8b93df18bc`；
- 隔离分支：`codex/meetily-i18n-phase5a4a`；
- 5A-4A 技术结论：**PASS**；
- 阶段 5 生产发布结论：**FAIL／NO-GO，维持不变**。

本阶段已经为受支持的升级／回滚通道建立了三层防线：应用内更新器只接受严格更高的 SemVer；当前 NSIS 安装器在交互和 `/S` 静默模式下都重新比较已安装版本并阻止降级；真正需要降级时，只能走具有安装包哈希、Authenticode、版本兼容快照、精确资源所有权和失败自动恢复的受控回滚工具。

最终离线 Windows Sandbox 已完成真实 `0.4.1 → 官方 0.3.0` 受控回滚。预检无写入，官方目标安装包签名有效，所有已声明残留按哈希精确清理，新版本地化模板残留为 0，目标版本数据快照逐文件恢复并校验，官方 0.3.0 安装后身份、签名和启动均通过。

这个 PASS 不代表任意旧安装器被“改造”。已经发布的官方 0.3.0 安装包不可被本项目追溯修改，直接手工运行它仍属于不受支持的危险旁路。生产候选仍未签名，真实历史用户数据、官方旧版运行时迁移证据以及阶段 5 的真机／治理门禁也未关闭，因此不得据此发布。

## 2. 审计范围和安全边界

### 2.1 纳入范围

- 应用内更新发现、下载和安装前的严格版本门禁；
- 当前 NSIS 安装器的交互／静默降级拦截；
- 0.3.0、0.4.0、0.4.1、0.4.2 的安装残留资源所有权清单；
- 未知文件、哈希不符文件、重解析点和非空未知目录的 fail-closed 行为；
- `%APPDATA%`、自定义模板仓库、配置和默认录音目录的受保护数据快照；
- 目标版本兼容快照校验、恢复后逐文件校验；
- 回滚前紧急快照、失败后的当前版本数据恢复和恢复安装器重装；
- 官方 0.3.0 签名安装包和未签名 0.4.1 审计候选的断网 Sandbox 实测；
- 前端、i18n、TypeScript、Next、Rust 和 PowerShell 回归；
- 正式 `target/release` 既有 EXE／NSIS 哈希保护。

### 2.2 明确不在范围内

- 未读取、复制或修改用户已安装的 `D:\桌面\开源版\meetily`；
- 未读取本机真实 Meetily 用户数据；
- 未覆盖 `D:\桌面\meetlily\target\release` 中的正式二进制；
- 未修改或重新签发历史官方 0.3.0 安装包；
- 未执行生产证书签名、时间戳、SmartScreen、真实渠道上传；
- 未宣称官方 0.3.0 二进制已经对历史 seed DB 留下运行时迁移证据；
- 未关闭真实历史备份、60 分钟录音、真实 Provider、读屏、非 Windows 平台和治理签字门禁。

## 3. 实施设计

### 3.1 三条版本通道

| 通道 | 允许行为 | 强制门禁 | 结果 |
|---|---|---|---|
| 应用内更新器 | 只允许当前版本到严格更高版本 | 完整 SemVer；发现和安装前各校验一次；无效、相等、旧版本全部拒绝 | PASS |
| 当前 NSIS 安装器 | 升级和同版本修复；禁止降级 | `allowDowngrades=false`；`NSIS_HOOK_PREINSTALL`；`SemverCompare`；静默模式同样 `SetErrorLevel 3` 后退出 | PASS |
| 受控回滚工具 | 经确认从较新版本回到已知较旧版本 | 明确 `-ConfirmRollback`、双安装包 SHA-256、默认强制有效签名、兼容快照、精确所有权、紧急恢复 | PASS |
| 直接执行任意历史安装器 | 不支持 | 历史二进制不可追溯修改，必须由发布运维策略禁止 | UNSAFE／PROHIBITED |

### 3.2 SemVer 门禁

`frontend/src/lib/semanticVersion.ts` 实现完整的 `major.minor.patch-prerelease+build` 解析和优先级比较。`isStrictlyNewerVersion(candidate, current)` 只在比较结果严格为 `1` 时返回 `true`；任一版本无法解析时返回 `false`。

`frontend/src/services/updateService.ts` 在两个时点执行相同门禁：

1. `checkForUpdates` 只把严格更高版本暴露为可用更新；
2. `downloadAndInstall` 在下载前重新读取当前版本并复验，失败时抛出受控错误 `UPDATE_VERSION_NOT_NEWER`。

这避免了“检查时合法、安装时版本状态已变化”的 TOCTOU 缺口，也避免服务端错误元数据触发同版本修复或降级。

### 3.3 安装资源所有权

清单：`install-resource-ownership.v1.json`；SHA-256：`102915C6B3AF8CC1D5474214CAC03095131B6E7F6F7A53A1661C92B57583D4D0`。

| 版本 | 精确文件数 | 目录数 | 说明 |
|---|---:|---:|---|
| 0.3.0 | 6 | 1 | 六个旧版根模板 |
| 0.4.0 | 18 | 3 | 六个继承根模板＋六个英文模板＋六个中文模板 |
| 0.4.1 | 18 | 3 | 显式复用 0.4.0 所有权集合 |
| 0.4.2 | 18 | 3 | 显式复用 0.4.0 所有权集合 |

每个文件都绑定相对路径和 SHA-256。工具先扫描完整安装目录，再决定是否清理：发现未声明文件、哈希不符、未知目录或重解析点时立即停止，且在停止前不删除被质疑的文件。目录只在清空后删除；受保护数据根不属于安装残留集合。

### 3.4 受保护数据

默认保护四个根：

- `%APPDATA%/com.meetily.ai`；
- `%APPDATA%/Meetily/templates`；
- `%APPDATA%/meetily`；
- `%USERPROFILE%/Music/meetily-recordings`。

快照清单绑定产品标识、已安装版本、原始绝对路径、文件相对路径、字节数和 SHA-256。恢复前必须满足目标版本相等、根映射相等、声明文件完整、无额外文件、无重解析点；恢复后再次逐项比对。

### 3.5 回滚事务和自动恢复

受控回滚按下列事务顺序执行：

1. 只读预检当前版本、目标版本、所有权、进程、安装包哈希／签名和目标兼容快照；
2. 创建当前较新版本的紧急数据快照；
3. 调用当前版本卸载器；
4. 仅清理当前版本清单中哈希完全匹配的安装残留；
5. 恢复目标版本兼容数据快照并校验；
6. 安装目标旧版本并核对注册版本；
7. 任一步失败时，尽力卸载部分目标版本、恢复紧急快照、重新安装冻结的当前版本恢复包；
8. 成功和失败都写结构化 JSON，成功退出码为 0，失败退出码为 1。

生产默认要求目标安装器和恢复安装器的 Authenticode 都为 `Valid`。`-AuditAllowUnsignedInstaller` 只为隔离审计候选保留，运行手册明确禁止在生产使用。

## 4. 静态与失败注入审计

### 4.1 静态审计

`audit-phase5a4-controlled-rollback.mjs` 最终结果为 **15/15 PASS**：

- 清单 schema、产品身份和四个保护根；
- unknown/hash mismatch/empty directory 三项 fail-closed 策略；
- 四个版本的所有权覆盖和别名无环；
- 60 次版本展开记录、24 个独立源文件路径的精确 SHA-256；
- 更新发现／安装双重严格升级门禁；
- SemVer 单元覆盖；
- 当前 NSIS 静默降级阻断；
- 资源清理重解析点和未知内容阻断；
- 快照版本、哈希、字节数和额外文件校验；
- 明确确认、生产签名默认值和审计专用未签名开关；
- 回滚前紧急快照和失败自动恢复路径；
- 失败注入覆盖；
- 最终 Sandbox 主报告与工具报告；
- 正式发布 EXE／NSIS 哈希不变。

证据：`docs/i18n/audit/phase-5a4/phase-5a4a-static-audit.json`。

### 4.2 PowerShell 失败注入

`test-phase5a4-controlled-rollback.ps1` 在 Windows PowerShell 5.1 和 PowerShell 7 两个宿主均通过 10/10：

| 断言 | 通过条件 | 结果 |
|---|---|---|
| 清单策略 | schema／产品／fail-closed 策略有效 | PASS |
| 版本比较 | 新／同／旧／预发布／无效版本行为正确 | PASS |
| 源资源哈希 | 0.4.x 18 个展开项全部匹配源码 | PASS |
| 精确清理 | 18 个已知文件和空目录全部移除 | PASS |
| 未知文件 | 阻断并保留未知文件 | PASS |
| 哈希不符 | 阻断并保留被修改文件 | PASS |
| 快照往返 | 捕获、校验、目标版本恢复正确 | PASS |
| 损坏快照 | 哈希变化后阻断恢复 | PASS |
| 重复文件声明 | 同一根内重复相对路径时阻断 | PASS |
| 重复根声明 | 重复根 ID 不能替代另一个保护根 | PASS |

### 4.3 真实失败恢复证据

第一次真实回滚时，清单只声明了 12 个本地化模板。工具发现 `templates/daily_standup.json` 未声明后按设计阻断，没有删除该文件；随后恢复 0.4.1 紧急数据快照并重装 0.4.1，`recoveredAfterFailure=true`。审计没有删除这次失败，而是把六个继承的根模板以精确哈希纳入 0.4.x 所有权后重新执行完整矩阵。

Windows Sandbox 启动方式、PowerShell 5.1 编码、注册表缺失属性、currentUser 用户上下文、子进程退出码和 Rust 测试环境的失败过程均记录在 `docs/i18n/audit/phase-5a4/failure-history.json`。

## 5. Windows Sandbox 严格验收

### 5.1 冻结输入

| 输入 | SHA-256 | 签名 |
|---|---|---|
| 官方 `meetily_0.3.0_x64-setup.exe` | `900C10A4EA05D991A06AB670886DE45DE08904903298A862A94FDC66F25420D9` | Valid，Zackriya Solutions Private Limited |
| 0.4.1 审计候选安装包 | `AD6D1B4B702873CBECBAD9CADD4F7166BC2C505DE70ACFCA226E1F8C46EF79EC` | NotSigned，仅显式审计开关允许 |
| WebView2 x64 Evergreen | `82B2D8A7013E0C0EA15D48FF4742EE3778BA16BD8B7B4A47876645B3E48D4016` | Valid |
| 资源所有权清单 | `102915C6B3AF8CC1D5474214CAC03095131B6E7F6F7A53A1661C92B57583D4D0` | 文件哈希冻结 |

### 5.2 最终流程

1. 断网安装 WebView2；
2. 安装官方 0.3.0；
3. 创建与 0.3.0 绑定的兼容数据快照；
4. 原位升级未签名 0.4.1 审计候选并启动存活 12 秒；
5. 执行只读 Preflight；
6. 创建 0.4.1 紧急快照；
7. 卸载 0.4.1，按精确所有权清理；
8. 恢复 0.3.0 快照并逐文件校验；
9. 安装官方 0.3.0，校验身份、签名和 12 秒启动；
10. 确认跨版本本地化残留为 0；
11. 写入报告并卸载 Sandbox 内最终版本。

### 5.3 最终结果

- 开始：`2026-08-23T18:31:39.4034997Z`；
- 完成：`2026-08-23T18:34:52.9558535Z`；
- 主报告：`passed=true`；
- 工具报告：`passed=true`、`mutated=true`、`recoveredAfterFailure=false`；
- Preflight：`passed=true`、`mutated=false`；
- 当前版本：0.4.1；目标版本：0.3.0；
- 0.4.1 卸载：Exit 0；
- 精确补充清理：六个继承根模板，安装根最终移除；
- 目标快照：恢复后逐文件等价；
- 官方 0.3.0 安装：Exit 0；
- 安装后 EXE SHA-256：`0C1A60560490CF78BC9C14303A1598DB0F05DC887A7E8EB7BD00B7EC04408045`；
- 安装后签名：`Valid`；
- 本地化跨版本残留：0；
- 回滚后启动：存活 12 秒。

本轮动态报告还绑定了最终脚本：受控回滚入口 SHA-256 `291FD791372672675107AA204D914EF42226E304B813CEB4D62DCBDBD0D4B554`，回滚模块 SHA-256 `7D2AFC9B830E4D2C1A3F0E3DAF719C7D7CAC6A99B41A7C77268193118F7B01B6`，Sandbox 审计脚本 SHA-256 `F96A8F0546B9D8B34DE0997AD852CA99E4E9BBCEC28B443B2994D20221E83F0C`。静态审计会把报告内这三个哈希与最终工作树文件再次比较。

六项最终断言全部为 `true`：冻结输入匹配、预检无写入、目标版本安装成功、跨版本本地化残留为空、兼容数据精确恢复、目标版本可启动。

证据：

- `docs/i18n/audit/phase-5a4/controlled-rollback-sandbox-final.json`；
- `docs/i18n/audit/phase-5a4/controlled-rollback-tool-final.json`。

## 6. 全库回归

| 门禁 | 最终结果 |
|---|---|
| ESLint | 0 error／0 warning |
| Node unit | 55/55 |
| i18n | 54/54 |
| TypeScript | Exit 0 |
| Next 静态页面 | 13/13 |
| PowerShell 5.1 失败注入 | 10/10 |
| PowerShell 7 失败注入 | 10/10 |
| Rust 支持测试目标 | 283 passed／0 failed／2 ignored，共 285 |

Rust 审计保留了两次环境失败。旧缓存 `app_lib` 测试 EXE 没有 Common Controls v6 manifest，因 `TaskDialogIndirect` 以 `0xC0000139` 退出；项目显式 `app_lib_tests` 目标经 `mt.exe` 验证已包含 v6 manifest。第一次执行受支持目标时有 282 通过、1 失败、2 忽略，唯一失败是 FFmpeg 不在测试进程 PATH；随后只把冻结 sidecar 暴露为 `ffmpeg.exe`，其 SHA-256 为 `5AF82A0D4FE2B9EAE211B967332EA97EDFC51C6B328CA35B827E73EAC560DC0D`，完整重跑得到 283/0/2。没有修改产品代码来掩盖测试环境问题。

机器可读汇总：`docs/i18n/audit/phase-5a4/regression-gates-final.json`。

## 7. 正式发布物保护

| 正式产物 | 冻结 SHA-256 | 最终 SHA-256 | 结果 |
|---|---|---|---|
| `target/release/meetily.exe` | `1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823` | 同左 | PASS |
| `target/release/bundle/nsis/meetily_0.4.0_x64-setup.exe` | `C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434` | 同左 | PASS |

0.4.1 未签名候选只进入一次性 Sandbox；正式目录没有被候选覆盖。

## 8. 分部分验收审计标准

| 编号 | 审计部分 | 强制通过标准 | 结果 |
|---|---|---|---|
| 5A4A-01 | 更新器门禁 | 发现和安装前均只接受严格更高 SemVer；无效值 fail-closed | PASS |
| 5A4A-02 | 当前安装器门禁 | 交互和 `/S` 模式都阻止降级并返回非零 | PASS（源码／静态） |
| 5A4A-03 | 所有权清单 | 0.3.0 与 0.4.x 全部残留文件路径、哈希、目录可追溯 | PASS |
| 5A4A-04 | 未知内容保护 | 未知、哈希不符、重解析点、未知目录均在删除前阻断 | PASS |
| 5A4A-05 | 数据隔离 | 四个默认保护根不属于安装资源清理范围 | PASS |
| 5A4A-06 | 快照合同 | 产品、版本、根映射、文件哈希／大小和额外文件严格校验 | PASS |
| 5A4A-07 | 回滚事务 | 明确确认、目标较旧、哈希／签名、卸载、清理、恢复、安装完整 | PASS |
| 5A4A-08 | 失败恢复 | 变更前紧急快照；失败后恢复数据并重装当前版本 | PASS（真实触发） |
| 5A4A-09 | Sandbox | 真实 0.4.1 → 官方 0.3.0，数据、残留、签名、身份、启动全通过 | PASS |
| 5A4A-10 | 自动回归 | 静态、PowerShell、前端、i18n、TS、Next、Rust 全通过 | PASS |
| 5A4A-11 | 失败证据 | 基础设施、实现和环境失败均保留并给出关闭依据 | PASS |
| 5A4A-12 | 正式产物保护 | 既有正式 EXE／NSIS 哈希不变 | PASS |
| 5A4A-13 | 生产签名 | 当前生产候选 EXE／NSIS 有有效 Authenticode 和时间戳 | **OPEN／阻断** |
| 5A4A-14 | 真实历史数据 | 经授权真实旧数据完成升级和回滚语义审计 | **OPEN／阻断** |

## 9. 缺陷和门禁状态

- `P5-DOWNGRADE-001`：**CLOSED（受支持通道）／历史安装器旁路 PROHIBITED**。应用内更新器和当前 NSIS 都禁止降级；受控回滚独立完成。无法追溯修改已经发布的旧安装器，直接手工执行旧包仍不属于受支持流程；
- `P5-INSTALL-OWNERSHIP-001`：**CLOSED**。0.3.0 和 0.4.x 精确路径＋SHA-256 所有权清单建立，跨版本残留为 0；
- `P5-ROLLBACK-RECOVERY-001`：**CLOSED（脱敏／沙箱）**。未知残留真实触发 fail-closed 后，紧急数据恢复和当前版本重装成功；
- `P5-UPG-RUNTIME-MIGRATION-001`：**OPEN／阻断**。官方旧版运行时迁移证据仍缺；
- `P5-UPG-REALDATA-001`：**OPEN／阻断**。未使用经授权真实历史备份；
- `P5-SIGN-001`：**OPEN／阻断**。0.4.1 审计候选仍为 `NotSigned`；
- 阶段 5 其他真机、长录音、Provider、读屏、平台矩阵和治理签字：**OPEN／阻断**。

## 10. 放行判断

5A-4A 的受控回滚与资源所有权隔离验收为 **PASS**，可以进入下一技术子阶段；但生产发布门禁继续为 **NO-GO**。下一优先项应是 **5A-4B：生产签名链与受控回滚包签名／时间戳验证**，随后再处理经授权真实历史数据和官方运行时迁移证明。

在生产发布前必须保持以下硬规则：

1. 不得直接运行旧版安装器降级；
2. 不得在生产使用 `-AuditAllowUnsignedInstaller`；
3. 未先捕获并验证目标版本兼容快照，不得回滚；
4. 出现未知残留、哈希不符、重解析点或签名不合法时必须停止；
5. 不得把本报告的 5A-4A PASS 解释为阶段 5 发布 PASS。
