# Meetily 阶段 5A-4B：生产签名链与受控回滚包签名严格审计报告

## 1. 审计结论

- 执行日期：2026-08-24；
- 隔离分支：`codex/meetily-i18n-phase5a4b`；
- 基线提交：`9c1713a75fbdd13e4f7e1b3e1bd32caf1fa90622`；
- 5A-4B 签名架构、门禁与负向测试：**PASS**；
- 既有 5A-4A 受控回滚回归：**PASS**；
- 真实生产签名执行：**未执行／外部条件不足**；
- 阶段 5 生产发布：**FAIL／NO-GO**。

本阶段完成的是可执行、默认拒绝、可审计的生产签名链。它不等于当前候选已经被生产证书签名。本机没有 `smctl`、DigiCert 凭据、代码签名证书私钥或 Tauri 更新私钥；策略中的 `productionActive` 证书指纹集合也有意保持为空。因此，生产构建会在签名前失败，不会继续生成可误认为“已签名发布”的产物。

## 2. 审计范围与保护边界

### 2.1 纳入范围

1. Tauri Windows `signCommand`；
2. DigiCert Software Trust Manager／KeyLocker CI 环境准备；
3. Authenticode 签名主体、证书指纹、代码签名 EKU、默认 Authenticode 信任链与时间戳链；
4. Tauri Ed25519／minisign 更新签名及公钥 ID；
5. 正式发布工作流、手动 Windows 工作流和 DevTest 工作流；
6. 受控回滚中的历史目标包与当前恢复包角色隔离；
7. 生产模式与显式非生产审计模式隔离；
8. 5A-4A 受控回滚的 PowerShell、静态和完整 Windows Sandbox 回归；
9. 正式冻结 EXE／NSIS 的只读哈希保护。

### 2.2 未执行事项

- 未访问任何生产 DigiCert 账户；
- 未创建、导入或使用自签名证书冒充生产证书；
- 未读取或输出任何私钥、API Key、证书口令或 KeyLocker 别名值；
- 未修改 `D:\桌面\开源版\meetily`；
- 未覆盖 `D:\桌面\meetlily\target\release` 中的正式二进制；
- 未把上游历史证书指纹自动批准为本中文发行版的当前生产证书；
- 未执行真实生产发布或 GitHub Release 上传。

## 3. 初始缺陷

| ID | 严重度 | 初始行为 | 风险 | 处置 |
|---|---|---|---|---|
| P5-SIGN-SKIP-001 | Critical | `DIGICERT_KEYPAIR_ALIAS` 缺失时 `sign-windows.ps1` 输出“Skipping”并 `exit 0` | 未签名构建可被 CI 当成成功 | 已关闭；生产默认 fail-closed |
| P5-SIGN-POLICY-002 | High | 只检查 `Get-AuthenticodeSignature.Status=Valid` | 任意受信发布者、错证书或缺少策略绑定仍可能通过 | 已关闭；主体、角色指纹、EKU、SignTool 与时间戳联合校验 |
| P5-SIGN-LOG-003 | High | CI 输出 API Key 前缀、KeyLocker 列表、别名和签名命令输出 | 凭据和密钥元数据暴露 | 已关闭；只输出是否配置及批准的公开证书身份 |
| P5-SIGN-CONTINUE-004 | High | 证书验证／同步失败后继续构建 | 失败被降级成警告 | 已关闭；全部改为非零失败 |
| P5-UPDATER-VERIFY-005 | High | 只上传 `.sig`，没有逐包密码学复验 | 空、错密钥或错包签名可能进入发布 | 已关闭；逐包校验 Ed25519 主签名、可信注释签名与 Key ID |
| P5-ROLLBACK-MODE-006 | High | `AuditAllowUnsignedInstaller` 没有独立安全模式 | 审计开关可能被误用于生产 | 已关闭；仅显式 `SecurityMode=Audit` 可用，生产传入立即失败 |
| P5-SIGN-CERT-007 | Blocking | 当前生产发布者主体／指纹未获批准，本机无生产凭据 | 无法完成真实生产签名 | **OPEN／外部阻断** |

## 4. 最终架构

### 4.1 单一签名策略

`windows-signing-policy.v1.json` 冻结以下内容：

- 产品名 `meetily` 与标识符 `com.meetily.ai`；
- 文件摘要算法 SHA-256；
- 代码签名 EKU `1.3.6.1.5.5.7.3.3`；
- 必须通过 Windows Default Authenticode 信任策略；
- 必须存在可验证的时间戳证书与时间戳信任链；
- 批准的发布者主体集合；
- `productionActive` 与 `upstreamHistorical` 两个证书指纹集合；
- Tauri 更新公钥 ID `ECA631D78797C82A`；
- 生产所需 DigiCert 和 Tauri 环境变量名称，但不保存任何值。

策略当前将 `productionActive` 保持为空。这是门禁，不是遗漏：当前发行主体和证书指纹未经发布负责人批准时，生产构建必须失败。

### 4.2 三种签名角色

| 角色 | 指纹集合 | 用途 | 当前状态 |
|---|---|---|---|
| `ProductionArtifact` | `productionActive` | 本次发布的应用和安装器 | LOCKED |
| `RecoveryInstaller` | `productionActive` | 回滚失败后恢复当前版本 | LOCKED |
| `HistoricalRollbackTarget` | `upstreamHistorical` | 冻结的上游历史目标安装器 | 可验证 |

历史目标白名单包含上游官方 0.3.0 的发布证书指纹 `0472869976D42A9F74D03B8B9CE60CF7A3983A3B`。它只允许作为历史回滚目标，不会自动成为本次生产发布证书。

### 4.3 Authenticode 联合门禁

`Meetily.Signing.psm1` 对每个 Windows 产物执行：

1. 原始文件 SHA-256（如调用方提供冻结值则精确比较）；
2. `Get-AuthenticodeSignature` 状态、主体、指纹、EKU 和时间戳证书提取；
3. Windows SDK `signtool verify /pa /all /tw /u 1.3.6.1.5.5.7.3.3 /q`；
4. 按角色选择指纹集合；
5. 任一条件不满足时返回结构化失败，生产调用使用 `ThrowOnFailure` 终止。

`/pa` 使用默认 Authenticode 策略；`/all` 验证全部嵌入签名；`/tw` 把缺少时间戳提升为警告退出；`/u` 要求代码签名 EKU。门禁只接受 SignTool 退出码 0，因此警告退出码 2 也不能发布。

### 4.4 DigiCert 签名前置检查

`prepare-digicert-signing.ps1` 在构建前检查：

- 全部必需环境变量非空；
- `smctl` 可用且 healthcheck 成功；
- `productionActive` 非空；
- CI 配置的证书指纹在生产白名单内；
- KeyLocker 证书同步成功；
- Windows 证书存储中存在该证书；
- 证书主体、代码签名 EKU和有效期符合策略。

KeyLocker 别名改为受保护的 `DIGICERT_KEYPAIR_ALIAS` Secret，不再通过 `smctl keypair ls` 枚举并打印。认证 P12 只写入 `RUNNER_TEMP`，工作流结束时验证路径仍在该目录内后删除。

### 4.5 签名命令

生产 `sign-windows.ps1`：

- 默认模式固定为 `Production`；
- 缺少凭据、`smctl`、生产指纹或指纹不匹配时非零失败；
- 使用 `smctl sign --tool=signtool --timestamp=true`，显式启用时间戳；
- 不输出 `smctl` 原始结果，避免泄漏凭据或 KeyLocker 元数据；
- 签名后立即运行完整策略验证。

非生产无签名构建只能使用 `AuditUnsigned`，并提供完整确认词 `I_UNDERSTAND_THIS_IS_NOT_FOR_RELEASE`。Tag 或 release 事件即使设置该模式也会失败。该路径只保证开发构建可继续，不会被正式发布工作流使用。

### 4.6 Tauri 更新签名

`verify-tauri-updater-signature.mjs` 不依赖外部 minisign 程序，使用 Node.js 原生密码学完成：

- 解析 Tauri 配置中的 minisign 公钥；
- 验证公钥、签名 Key ID 均为 `ECA631D78797C82A`；
- 对安装包计算 BLAKE2b-512；
- 验证 Ed25519 预哈希主签名；
- 验证可信注释的全局签名；
- 任一签名、Key ID 或封装格式错误时非零失败。

`audit-phase5a4b-release-signing.ps1` 逐个检查 bundle 中的 EXE／MSI，要求 Authenticode 策略通过、同名 `.sig` 存在且非空、Ed25519 主签名和可信注释签名均通过。正式 CI 还要求 `TAURI_SIGNING_PRIVATE_KEY` 与口令环境变量存在。

### 4.7 CI 工作流

三条 Windows 构建路径统一具备：

1. 安全地把 GitHub Secrets 映射为环境变量；
2. 生产签名前置策略检查；
3. Tauri 构建阶段显式 `Production`／`AuditUnsigned`；
4. 构建后逐产物双签名门禁；
5. 认证证书材料清理；
6. 禁止输出 API Key 前缀、KeyLocker 列表和别名值。

`.github/workflows/release.yml` 调用正式可复用工作流时固定 `sign-binaries: true`，不能通过正式 release 路径请求无签名构建。

## 5. 真实样本证据

### 5.1 上游官方 0.3.0 正样本

| 项目 | 值 |
|---|---|
| 安装包 SHA-256 | `900C10A4EA05D991A06AB670886DE45DE08904903298A862A94FDC66F25420D9` |
| Authenticode | `Valid` |
| 发布者 | `CN=Zackriya Solutions Private Limited, O=Zackriya Solutions Private Limited, L=Bengaluru, S=Karnataka, C=IN` |
| 发布证书指纹 | `0472869976D42A9F74D03B8B9CE60CF7A3983A3B` |
| 发布证书有效期（UTC） | 2025-10-14 至 2028-10-13 |
| EKU | `1.3.6.1.5.5.7.3.3` |
| 时间戳证书 | DigiCert SHA256 RSA4096 Timestamp Responder 2025 1 |
| 时间戳证书指纹 | `DD6230AC860A2D306BDA38B16879523007FB417E` |
| SignTool `/pa /all /tw /u` | Exit 0 |
| Tauri 更新主签名 | Valid |
| 可信注释签名 | Valid |
| 更新签名 Key ID | `ECA631D78797C82A` |

该样本通过 `HistoricalRollbackTarget`，但因生产角色指纹集合为空，不会通过 `ProductionArtifact`。角色负向结果符合设计。

### 5.2 当前 0.4.1 审计候选负样本

| 项目 | 结果 |
|---|---|
| SHA-256 | `AD6D1B4B702873CBECBAD9CADD4F7166BC2C505DE70ACFCA226E1F8C46EF79EC` |
| Authenticode | `NotSigned` |
| 时间戳 | 无 |
| 同名 Tauri `.sig` | 无 |
| 生产发布门禁 | Exit 1／FAIL |
| 文件在失败后 | 哈希不变 |

### 5.3 篡改负样本

在临时副本末尾追加 1 字节后：

- Authenticode／SignTool 门禁失败；
- Tauri Ed25519 主签名失败；
- 可信注释签名本身仍可验证，但主签名失败使总结果为 FAIL；
- 原始上游安装包未被修改。

## 6. 测试与验收

| 审计项 | 标准 | 结果 |
|---|---|---|
| 签名策略静态审计 | 策略、脚本、工作流、回滚和正式哈希全部通过 | 34/34 PASS |
| PowerShell 7 签名测试 | 全部分支和真实样本无失败 | 18/18 PASS |
| Windows PowerShell 5.1 签名测试 | 与 PS7 等价 | 18/18 PASS |
| 签名命令生产缺凭据 | 非零失败且候选哈希不变 | PASS |
| 显式审计无签名模式 | 非 Tag／Release 可用且不修改文件 | PASS |
| Tag／Release 审计旁路 | 非零失败 | PASS |
| 上游真实 Authenticode | 历史角色、EKU、链和时间戳通过 | PASS |
| 上游真实更新签名 | 主签名、可信注释、Key ID 通过 | PASS |
| 篡改样本 | Authenticode 与更新主签名均拒绝 | PASS |
| 当前未签名发布门禁 | 逐包拒绝且无 `.sig` | PASS（预期负向） |
| 工作流 YAML | 四个工作流可解析 | PASS |
| 5A-4A PowerShell 回归 | PS7／PS5.1 各 10 项 | 10/10 × 2 PASS |
| 5A-4A 静态回归 | 最终脚本哈希与 Sandbox 证据一致 | 15/15 PASS |
| Windows Sandbox 回滚 | 六项最终断言全真 | 6/6 PASS |
| 前端单元回归 | 无失败 | 55 passed／0 failed |
| i18n 回归 | 无失败 | 54 passed／0 failed |
| 最终聚合门禁 | 全部技术断言通过 | 14/14 PASS |

### 6.1 最终 Windows Sandbox

最终断网 Sandbox 使用本阶段最终版回滚脚本：

- 运行时间：2026-08-23T19:01:50Z 至 19:05:11Z；
- 冻结输入哈希：全部匹配；
- Preflight：通过且 `mutated=false`；
- 回滚工具：`SecurityMode=Audit`、`passed=true`、`mutated=true`；
- 目标版本：官方 0.3.0；
- 目标 EXE SHA-256：`0C1A60560490CF78BC9C14303A1598DB0F05DC887A7E8EB7BD00B7EC04408045`；
- 目标 EXE 签名：`Valid`；
- 跨版本本地化残留：0；
- 数据恢复：逐文件精确一致；
- 启动探针：存活 12 秒；
- 六项断言：全部 `true`。

Sandbox 必须使用 `Audit`，因为当前 0.4.1 恢复包未签名。生产模式拒绝无签名恢复包的分支已经由真实文件测试和签名命令子进程测试覆盖；不能把 Sandbox 功能回滚 PASS 解释为生产签名 PASS。

## 7. 分部分验收审计标准

| 部分 | 强制验收标准 | 实际状态 |
|---|---|---|
| 策略冻结 | 算法、主体、指纹集合、角色、EKU、时间戳和更新 Key ID 可审查 | PASS |
| 生产默认拒绝 | 未配置、错指纹、空生产集合、签名失败均非零退出 | PASS |
| 证书身份 | 主体与角色指纹同时匹配，单独 `Valid` 不足以通过 | PASS |
| 信任链／时间戳 | SignTool Default Authenticode、全部签名、时间戳、EKU退出码必须为 0 | PASS |
| 更新签名 | 每个发布包的主签名、可信注释签名和 Key ID 均通过 | PASS |
| 工作流保密 | 不输出 Secret 值、前缀、KeyLocker 列表或别名 | PASS |
| 审计模式隔离 | 明确模式和确认词；Tag／Release 禁止；生产回滚禁止旁路 | PASS |
| 回滚角色 | 历史目标和当前恢复包使用不同指纹集合 | PASS |
| 故障不变更 | 签名前置失败不修改待签文件；回滚 Preflight 不变更系统 | PASS |
| 正式产物保护 | 正式 EXE／NSIS 哈希不变 | PASS |
| 真实生产凭据 | 批准证书、KeyLocker、更新私钥可用 | OPEN／阻断 |
| 真实签名 RC | 当前中文 EXE、NSIS、`.sig` 和 `latest.json` 通过门禁 | OPEN／阻断 |

## 8. 正式产物保护

| 产物 | 冻结 SHA-256 | 最终 SHA-256 | 判定 |
|---|---|---|---|
| `D:\桌面\meetlily\target\release\meetily.exe` | `1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823` | 相同 | PASS |
| `D:\桌面\meetlily\target\release\bundle\nsis\meetily_0.4.0_x64-setup.exe` | `C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434` | 相同 | PASS |

本阶段只修改隔离工作树中的策略、脚本、CI 和文档；既有正式二进制没有被签名、替换或覆盖。

## 9. 阻断项与发布决定

### 9.1 已关闭

- `P5-SIGN-SKIP-001`：生产签名缺配置时静默成功；
- `P5-SIGN-POLICY-002`：只检查 `Status=Valid`；
- `P5-SIGN-LOG-003`：CI 输出敏感元数据；
- `P5-SIGN-CONTINUE-004`：证书失败继续构建；
- `P5-UPDATER-VERIFY-005`：不做更新签名密码学复验；
- `P5-ROLLBACK-MODE-006`：未签名审计旁路缺少安全模式隔离。

### 9.2 继续阻断

- `P5-SIGN-001`：**OPEN**。没有获批的当前生产发布者主体／证书指纹，没有本地生产凭据，没有真实签名中文 RC；
- `P5-UPG-RUNTIME-MIGRATION-001`：**OPEN**；
- `P5-UPG-REALDATA-001`：**OPEN**；
- 平台矩阵、60 分钟录音、真实 Provider、人审、读屏、原生 Shell 和治理签字仍按阶段 5 总门禁保持 OPEN。

### 9.3 决定

5A-4B 的签名架构和自动门禁验收为 **PASS**，但真实生产签名未发生，因此阶段 5 发布决定仍为 **NO-GO**。不得使用上游 0.3.0 的签名正样本、Audit Sandbox PASS 或模拟证据替代当前生产中文 RC 的签名证据。

下一步建议进入 **5A-4C：受控批准当前发布证书并执行真实签名 RC 门禁**。该阶段只有在发布负责人提供合法的发布者主体、证书指纹、DigiCert/SMCTL 凭据和 Tauri 更新私钥后才能完成；随后再处理真实历史数据和官方运行时迁移证明。

## 10. 证据索引

- `docs/i18n/phase-5a4/windows-signing-policy.v1.json`；
- `docs/i18n/scripts/Meetily.Signing.psm1`；
- `frontend/src-tauri/scripts/prepare-digicert-signing.ps1`；
- `frontend/src-tauri/scripts/sign-windows.ps1`；
- `docs/i18n/scripts/audit-phase5a4b-release-signing.ps1`；
- `docs/i18n/scripts/verify-tauri-updater-signature.mjs`；
- `docs/i18n/scripts/test-phase5a4b-signing-policy.ps1`；
- `docs/i18n/scripts/audit-phase5a4b-production-signing.mjs`；
- `docs/i18n/scripts/capture-phase5a4b-final-evidence.mjs`；
- `docs/i18n/phase-5a4/phase-5a4b-production-signing-runbook.zh-CN.md`；
- 正式交付目录中的 `phase5a4b-final-evidence.json`、双 PowerShell 测试、Sandbox 报告、更新签名证据、静态门禁与回归日志。
