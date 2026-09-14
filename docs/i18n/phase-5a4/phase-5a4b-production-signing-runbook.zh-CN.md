# Meetily Windows 生产签名与受控回滚签名操作手册（阶段 5A-4B）

## 1. 用途

本手册用于：

- 批准或轮换 Windows 生产代码签名证书；
- 配置 DigiCert Software Trust Manager／KeyLocker 与 Tauri 更新签名；
- 构建并验证真实签名 RC；
- 验证受控回滚的历史目标包和当前恢复包；
- 在签名、时间戳或更新签名异常时安全停止发布。

本手册不授权创建自签名证书、导出生产私钥、绕过组织审批或把 `AuditUnsigned` 产物发布给用户。

## 2. 安全原则

1. 生产默认 fail-closed；
2. 代码签名私钥保留在批准的 HSM／KeyLocker；
3. 当前发布证书必须同时匹配主体和 SHA-1 指纹白名单；
4. Authenticode 和 Tauri 更新签名是两条独立必需链；
5. 每个 Windows 发布产物都必须有受信时间戳；
6. 历史回滚目标与当前生产／恢复包使用不同角色；
7. `AuditUnsigned` 只允许非 Tag、非 Release 的开发构建；
8. 日志不得输出 API Key、口令、私钥、KeyLocker 列表或别名值；
9. 任一门禁失败即停止，不上传、不重命名为正式包、不修改 `latest.json`。

## 3. 生产证书批准

发布负责人必须先从受控证书源取得以下公开元数据：

- 完整 Subject Distinguished Name；
- SHA-1 Thumbprint；
- NotBefore／NotAfter；
- EKU 中必须包含 `1.3.6.1.5.5.7.3.3`；
- 对应 KeyLocker Keypair Alias；
- 证书状态、撤销状态和组织审批单号。

只允许在评审变更中修改：

```text
docs/i18n/phase-5a4/windows-signing-policy.v1.json
```

批准新发行主体时，将完整主体加入 `approvedSignerSubjects`；将当前证书指纹加入 `signerThumbprintSets.productionActive`。不得删除仍需验证的历史证书；历史证书只能放入 `upstreamHistorical` 并只授予 `HistoricalRollbackTarget`。

变更必须由至少两人复核：发布负责人确认发行主体，安全负责人确认指纹、EKU、有效期和审批来源。

## 4. GitHub Secrets

正式工作流需要：

| Secret | 用途 |
|---|---|
| `SM_HOST` | DigiCert 服务端点 |
| `SM_API_KEY` | DigiCert API 认证 |
| `SM_CLIENT_CERT_FILE_B64` | 客户端认证 P12 的 Base64 |
| `SM_CLIENT_CERT_PASSWORD` | 客户端认证 P12 口令 |
| `SM_CODE_SIGNING_CERT_SHA1_HASH` | 当前生产代码签名证书指纹 |
| `DIGICERT_KEYPAIR_ALIAS` | 获批 KeyLocker Keypair Alias |
| `TAURI_SIGNING_PRIVATE_KEY` | Tauri 更新签名私钥 |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Tauri 更新签名私钥口令 |

这些值只能配置在受保护的 GitHub Environment／Repository Secrets 中。不得写入仓库、命令行记录、PR 评论、构建摘要或审计 JSON。

## 5. 证书前置检查

CI 自动运行：

```powershell
./frontend/src-tauri/scripts/prepare-digicert-signing.ps1
```

通过条件：

- 必需环境变量全部存在；
- `smctl healthcheck` 成功；
- `productionActive` 至少一个指纹；
- `SM_CODE_SIGNING_CERT_SHA1_HASH` 在生产集合中；
- 证书同步成功；
- Windows 证书存储找到完全相同指纹；
- 主体、EKU、有效期符合策略。

任一条件失败时，不允许手工调用 `smctl sign` 绕过前置检查。

## 6. 正式构建

`.github/workflows/release.yml` 固定调用：

```yaml
sign-binaries: true
```

Tauri Windows 构建阶段应看到：

```text
MEETILY_WINDOWS_SIGNING_MODE=Production
```

签名命令由 Tauri `signCommand` 自动调用：

```powershell
./frontend/src-tauri/scripts/sign-windows.ps1 -FilePath <artifact>
```

脚本会显式要求 DigiCert 时间戳，签名后立即执行策略验证。禁止把原始 `smctl` 输出添加回 CI 日志。

## 7. 构建后发布门禁

正式工作流自动执行：

```powershell
./docs/i18n/scripts/audit-phase5a4b-release-signing.ps1 `
  -ArtifactRoot <bundle-directory> `
  -RequireUpdaterEnvironment `
  -ReportPath <audit-json>
```

每个 EXE／MSI 必须同时满足：

1. SHA-256 可计算；
2. Authenticode `Valid`；
3. 发布者主体获批；
4. 证书指纹属于 `productionActive`；
5. 代码签名 EKU 存在；
6. SignTool `/pa /all /tw /u` Exit 0；
7. 时间戳证书存在；
8. 同名 `.sig` 存在且非空；
9. Ed25519 主签名有效；
10. 可信注释签名有效；
11. Key ID 为 `ECA631D78797C82A`。

正式 Release 还必须检查 `latest.json` 中的 URL、版本、平台、签名字符串与实际上传资产一致。任何上传后重命名或二进制改动都会破坏签名，应重新构建和签名，不得手工修补。

## 8. 独立验证命令

### 8.1 Authenticode

```powershell
Import-Module ./docs/i18n/scripts/Meetily.Signing.psm1 -Force

Test-MeetilySignedFile `
  -Path <installer.exe> `
  -PolicyPath ./docs/i18n/phase-5a4/windows-signing-policy.v1.json `
  -Role ProductionArtifact `
  -ExpectedSha256 <frozen-sha256> `
  -ThrowOnFailure
```

### 8.2 Tauri 更新签名

```powershell
node ./docs/i18n/scripts/verify-tauri-updater-signature.mjs `
  --artifact <installer.exe> `
  --signature <installer.exe.sig> `
  --tauri-config ./frontend/src-tauri/tauri.conf.json
```

退出码必须为 0，且三项断言均为 `true`。

## 9. 生产受控回滚

### 9.1 Preflight

生产回滚必须保持默认 `SecurityMode=Production`，不得传 `AuditAllowUnsignedInstaller`：

```powershell
./docs/i18n/scripts/invoke-meetily-controlled-rollback.ps1 `
  -Mode Preflight `
  -SnapshotRoot <target-compatible-snapshot> `
  -TargetInstaller <historical-installer> `
  -ExpectedTargetInstallerSha256 <historical-sha256> `
  -TargetVersion <historical-version> `
  -RecoveryInstaller <current-signed-installer> `
  -ExpectedRecoveryInstallerSha256 <current-sha256> `
  -AuditReportPath <preflight-report.json>
```

通过条件：

- 历史包通过 `HistoricalRollbackTarget`；
- 当前恢复包通过 `RecoveryInstaller`；
- 快照版本与目标版本匹配；
- `passed=true`；
- `mutated=false`。

### 9.2 Rollback

只有 Preflight 报告通过后才可执行，并必须额外提供：

- 独立的紧急备份目录；
- `-ConfirmRollback`；
- 经批准的变更单／事故单；
- 当前恢复包仍可获取且哈希不变。

生产命令不得出现：

```text
-SecurityMode Audit
-AuditAllowUnsignedInstaller
```

## 10. AuditUnsigned 边界

允许用途：

- 本地开发构建；
- 隔离 CI 的非正式测试；
- 断网 Windows Sandbox 功能回滚验证。

禁止用途：

- Git Tag；
- GitHub Release；
- 对外分发；
- 安装到生产用户环境；
- 生成或更新正式 `latest.json`；
- 作为代码签名验收证据。

AuditUnsigned 需要两个环境变量同时精确匹配：

```text
MEETILY_WINDOWS_SIGNING_MODE=AuditUnsigned
MEETILY_ALLOW_UNSIGNED_WINDOWS_BUILD=I_UNDERSTAND_THIS_IS_NOT_FOR_RELEASE
```

## 11. 证书轮换与撤销

### 11.1 正常轮换

1. 新证书完成组织审批；
2. 将新主体／指纹加入策略；
3. 保留旧生产指纹直到所有仍受支持的已发布包完成验证周期；
4. 使用新证书签一个隔离 RC；
5. 运行完整双签名门禁和安装／更新／回滚矩阵；
6. 发布负责人和安全负责人签字；
7. 再移除不再允许签当前发布物的旧生产指纹；
8. 如旧包仍作为历史目标，将旧指纹移入历史集合，而不是彻底删除证据。

### 11.2 撤销或泄漏

1. 立即停止所有发布工作流；
2. 撤销／禁用 KeyLocker Keypair 和相关凭据；
3. 轮换 API Key、客户端认证证书和别名 Secret；
4. 从 `productionActive` 移除被撤销指纹；
5. 保留事故时点、受影响版本和审计日志；
6. 使用新证书重新签名并发布新版本；
7. 评估 Tauri 更新私钥是否同时受影响；若受影响，按 Tauri 密钥迁移方案处理，不能只替换代码签名证书。

## 12. 故障处置

| 故障 | 动作 |
|---|---|
| `productionActive` 为空 | 停止；完成证书主体和指纹审批 |
| 配置指纹不在策略 | 停止；核对 Secret 和策略，不自动放宽 |
| `smctl healthcheck` 失败 | 停止；检查 DigiCert 账户、认证证书和网络 |
| 证书同步失败 | 停止；不回退到本地自签名 |
| 主体／EKU／有效期失败 | 停止；更换正确证书或修正审批记录 |
| SignTool Exit 2 | 视为失败；通常为时间戳或 EKU警告 |
| `.sig` 缺失 | 停止；核对 Tauri 更新私钥配置 |
| Ed25519 主签名失败 | 停止；资产已变化或签名不匹配 |
| 可信注释签名失败 | 停止；签名封装不可接受 |
| Key ID 不匹配 | 停止；不得临时替换应用内公钥 |
| 回滚生产模式拒绝恢复包 | 提供真正签名的当前恢复包，不启用 Audit 旁路 |

## 13. 发布验收记录

每次真实签名 RC 至少保存：

- 源提交、Tag、版本和构建运行 ID；
- 策略文件 SHA-256；
- 证书主体、指纹、有效期、EKU；
- 每个产物的 SHA-256、Authenticode 和时间戳证据；
- 每个 `.sig` 的 SHA-256、Key ID、主签名和可信注释结果；
- `latest.json` SHA-256 与资产映射；
- 安装、更新、卸载、受控回滚报告；
- 发布负责人、安全负责人和中文验收人签字；
- 最终 GO／NO-GO 决议。

只有全部强制项通过，才可将 5A-4C 和 `P5-SIGN-001` 标记为 CLOSED。
