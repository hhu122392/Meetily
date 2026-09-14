# Meetily Windows 生产证书准入操作手册（5A-4C-1）

## 1. 目的和安全边界

本手册用于把当前 Windows 发布证书纳入 `productionActive`。它不负责签发证书，不生成生产私钥，也不允许用自签名证书或历史上游证书代替当前生产证书。准入成功只允许进入真实签名 RC 阶段，不代表可直接发布。

任何时候都不得把以下值写入源码、准入 JSON、审计文档、命令行参数、工单正文或构建日志：

- `SM_API_KEY`；
- `SM_CLIENT_CERT_PASSWORD`；
- P12 内容或非临时路径；
- `DIGICERT_KEYPAIR_ALIAS` 的实际值；
- `TAURI_SIGNING_PRIVATE_KEY`；
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。

证书 Subject、Issuer、Serial、SHA-1 Thumbprint、有效期和 EKU 属于准入所需公开证据，可以进入受控记录。

## 2. 状态机

```text
PendingCandidateEvidence
  → PendingApproval
  → Approved
  → 真实签名 RC
  → 发布门禁 GO

任一步不一致：Rejected
证书泄露、吊销或停用：Revoked
```

`Approved` 必须同时满足：候选公开证据完整、证书质量通过、DigiCert 通道通过、一次性持钥证明通过、Tauri 私钥入口存在、发布负责人批准、安全复核人批准、`productionActive` 只含该候选指纹。

## 3. 非破坏性本机预审

在仓库根目录运行：

```powershell
pwsh -NoProfile -File .\docs\i18n\scripts\audit-phase5a4c1-certificate-admission.ps1 `
  -ReportPath .\target\phase5a4c1-local-preflight.json
```

默认不会调用 DigiCert，不会同步证书，不会读取私钥，也不会签名。报告中的环境部分只有 `true/false`。

需要把缺失条件作为流水线失败时使用：

```powershell
pwsh -NoProfile -File .\docs\i18n\scripts\audit-phase5a4c1-certificate-admission.ps1 `
  -ReportPath .\target\phase5a4c1-enforced-preflight.json `
  -EnforceReady
```

未满足全部准入条件时必须返回非零。

## 4. 候选证书证据

受控环境应提供 DigiCert 六项环境入口和 Tauri 两项私钥入口。客户端 P12 只允许位于 CI 临时目录，任务结束必须删除。工具执行期间不打印环境值，不运行 `smctl keypair ls/list`。

准入记录中的候选字段必须来自同步后证书本身：

- `subject`：必须精确等于签名策略允许的发布者主体；
- `sha1Thumbprint`：40 位大写十六进制；
- `serialNumber`：证书序列号；
- `issuer`：公共 CA 颁发者；
- `notBeforeUtc`／`notAfterUtc`：ISO 8601 UTC；
- EKU：必须含 `1.3.6.1.5.5.7.3.3`；
- 链：在线撤销检查和默认信任链均通过；
- 剩余有效期：至少 30 天；
- 指纹：不得出现在 `upstreamHistorical`。

证据不足时保持 `PendingCandidateEvidence`。证据完整但审批未齐时只允许进入 `PendingApproval`。

## 5. DigiCert 连接和一次性持钥证明

在准入记录已经填入待审候选公开字段、受控环境变量已注入后，使用一个可丢弃的非发布 PE 输入：

```powershell
pwsh -NoProfile -File .\docs\i18n\scripts\audit-phase5a4c1-certificate-admission.ps1 `
  -RunDigiCertConnectivity `
  -ProofOfPossessionPePath .\path\to\disposable-input.exe `
  -ReportPath .\target\phase5a4c1-connected-preflight.json
```

脚本会：

1. 运行 `smctl healthcheck`；
2. 按环境中的别名执行证书同步，但不打印别名；
3. 按配置指纹从 Windows 证书库定位候选；
4. 把输入复制到固定前缀的临时目录；
5. 对临时副本执行带时间戳的 DigiCert 签名；
6. 用 Authenticode 和 Windows SDK SignTool 复验主体、指纹、EKU、链和时间戳；
7. 安全删除临时副本；
8. 原始输入和正式产物均不修改。

禁止把正式安装包当作 5A-4C-1 的试签输入；正式安装包只能在后续 RC 门禁中签名。

## 6. 双审批和策略更新

发布负责人核对证书归属、产品发布授权、有效期和候选公开字段；安全复核人独立核对证书链、指纹、历史隔离、持钥证明、日志脱敏和 Tauri Key ID。两者不得使用同一个审批身份。

每项审批必须包含：

- `status: Approved`；
- UTC 批准时间；
- 可追踪且不含秘密的审批引用。

审批完成后，受审变更必须同时满足：

- 准入记录状态为 `Approved`；
- `productionActive` 仅有一个指纹；
- 该指纹等于准入候选；
- 现有历史指纹集合未改变；
- 源码审查可以清楚看到候选和审批变更；
- 自动测试和静态审计全绿。

任何只修改 `productionActive` 而没有匹配准入记录和双审批的变更都会被 `prepare-digicert-signing.ps1` 拒绝。

## 7. 轮换、拒绝和吊销

- 候选证书证据不一致：状态设为 `Rejected`，不得写入活动集合；
- 证书私钥疑似泄露、证书吊销或 DigiCert Keypair 停用：状态立即改为 `Revoked`，从 `productionActive` 删除；
- 证书轮换：创建新的受审候选，不在同一次发布中保留多个活动指纹；
- 历史签名证书：只能保留在对应历史回滚集合，不能重新提升为当前生产证书；
- 已签名产物：冻结 SHA-256、签名时间戳和证书证据，不能用重新签名悄悄替换同版本文件。

## 8. 准入完成定义

只有以下条件全部为真，5A-4C-1 的真实准入才可判定 GO：

- 准入记录 `Approved`；
- 双审批完整且可追踪；
- 唯一 `productionActive` 与候选一致；
- 公共证书主体、指纹、序列号、Issuer、EKU、链、有效期通过；
- DigiCert healthcheck 和 certsync 通过；
- 一次性 PE 的真实持钥证明、时间戳和 SignTool 通过；
- Tauri Key ID 等于 `ECA631D78797C82A`，对应私钥入口存在；
- 秘密泄露扫描为零；
- 双 PowerShell 测试、静态审计和旧阶段回归全部通过。

完成准入后必须进入 5A-4C-2 构建全新 RC；没有真实签名 RC、`.sig`、`latest.json`、安装／升级／回滚验证之前，生产发布仍是 NO-GO。
