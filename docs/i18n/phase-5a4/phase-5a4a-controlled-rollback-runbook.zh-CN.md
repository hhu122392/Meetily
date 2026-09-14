# Meetily 受控回滚操作手册（阶段 5A-4A）

## 1. 适用范围

本手册只用于 Meetily `com.meetily.ai` 的受控版本回滚。它不是普通“覆盖安装”说明，也不授权直接运行旧版安装器。直接用历史安装包覆盖较新版本会绕过当前版本的防护，并可能遗留跨版本模板。

生产执行必须同时具备：

- 当前版本和目标版本都在 `install-resource-ownership.v1.json` 中；
- 目标安装器、当前版本恢复安装器的预期 SHA-256；
- 两个安装器的 Authenticode 状态都为 `Valid`；
- 在目标版本仍运行正常时创建的、与目标版本严格绑定的数据快照；
- 应用进程全部关闭；
- 独立且空间充足的紧急快照目录；
- 可写的 JSON 审计报告路径。

## 2. 文件

| 文件 | 用途 |
|---|---|
| `docs/i18n/scripts/Meetily.Rollback.psm1` | SemVer、清单、残留清理、安装包证据、快照和恢复原语 |
| `docs/i18n/scripts/invoke-meetily-controlled-rollback.ps1` | 捕获、预检和回滚事务入口 |
| `docs/i18n/phase-5a4/install-resource-ownership.v1.json` | 版本化安装资源所有权和受保护数据根 |
| `docs/i18n/scripts/test-phase5a4-controlled-rollback.ps1` | 非生产失败注入测试 |

## 3. 第一步：在目标旧版本时期捕获兼容快照

兼容快照必须在目标版本仍安装且数据可读时提前创建。示例中的路径和哈希必须替换成变更单冻结值：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\docs\i18n\scripts\invoke-meetily-controlled-rollback.ps1 `
  -Mode CaptureSnapshot `
  -OwnershipManifestPath .\docs\i18n\phase-5a4\install-resource-ownership.v1.json `
  -SnapshotRoot D:\MeetilyRollback\snapshots\v0.3.0-compatible `
  -AuditReportPath D:\MeetilyRollback\reports\capture-v0.3.0.json
```

验收：退出码 0；报告 `passed=true`、`mutated=false`；快照清单 `installedVersion` 等于目标版本；快照清单 SHA-256登记到变更单。

## 4. 第二步：只读预检

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\docs\i18n\scripts\invoke-meetily-controlled-rollback.ps1 `
  -Mode Preflight `
  -OwnershipManifestPath .\docs\i18n\phase-5a4\install-resource-ownership.v1.json `
  -SnapshotRoot D:\MeetilyRollback\snapshots\v0.3.0-compatible `
  -TargetVersion 0.3.0 `
  -TargetInstaller D:\MeetilyRollback\installers\meetily_0.3.0_x64-setup.exe `
  -ExpectedTargetInstallerSha256 <64位SHA-256> `
  -RecoveryInstaller D:\MeetilyRollback\installers\meetily_current_x64-setup.exe `
  -ExpectedRecoveryInstallerSha256 <64位SHA-256> `
  -AuditReportPath D:\MeetilyRollback\reports\preflight.json
```

验收：退出码 0；`passed=true`；`mutated=false`；`strictDowngradeConfirmed=true`；两个版本的所有权均已知；`compatibleSnapshotVerified=true`；`appProcessesClosed=true`；`signaturesRequired=true`。

任一项不满足都必须停止，不得转入 Rollback。

## 5. 第三步：执行受控回滚

紧急快照目录必须不存在，避免覆盖以前证据：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass `
  -File .\docs\i18n\scripts\invoke-meetily-controlled-rollback.ps1 `
  -Mode Rollback `
  -ConfirmRollback `
  -OwnershipManifestPath .\docs\i18n\phase-5a4\install-resource-ownership.v1.json `
  -SnapshotRoot D:\MeetilyRollback\snapshots\v0.3.0-compatible `
  -EmergencyBackupRoot D:\MeetilyRollback\snapshots\emergency-current-<变更号> `
  -TargetVersion 0.3.0 `
  -TargetInstaller D:\MeetilyRollback\installers\meetily_0.3.0_x64-setup.exe `
  -ExpectedTargetInstallerSha256 <64位SHA-256> `
  -RecoveryInstaller D:\MeetilyRollback\installers\meetily_current_x64-setup.exe `
  -ExpectedRecoveryInstallerSha256 <64位SHA-256> `
  -AuditReportPath D:\MeetilyRollback\reports\rollback.json
```

成功标准：

- 退出码 0；
- `passed=true`、`mutated=true`、`recoveredAfterFailure=false`；
- 当前版本卸载器 Exit 0；
- 精确所有权清理没有未知文件或哈希不符；
- `targetDataSnapshotRestored=true`；
- 目标安装器 Exit 0；
- `installedTargetVersion` 等于目标版本。

随后必须人工复核应用启动、会议列表、旧会议、录音路径、模板仓库和设置，并把结果附到变更单。

## 6. 失败行为

Rollback 模式在任何实际变更前创建当前版本紧急快照。如果卸载、残留清理、旧数据恢复或目标安装失败，工具会按顺序尽力：

1. 移除已部分安装的目标版本；
2. 恢复当前版本紧急数据快照；
3. 运行冻结的当前版本恢复安装器；
4. 在 JSON 中写入恢复动作和错误；
5. 以退出码 1 结束。

`recoveredAfterFailure=true` 只表示自动恢复路径完成，不表示原回滚成功。此时必须停止变更、保留两个快照和报告，不得立即再次回滚。

## 7. 必须阻断的情况

- 当前版本不是清单已知版本；
- 目标版本不严格低于当前版本；
- 任一安装包 SHA-256 不匹配；
- 任一生产安装包 Authenticode 不是 `Valid`；
- 目标快照版本、根映射、字节数或哈希不匹配；
- 快照或安装目录包含重解析点；
- 安装目录出现清单未声明文件／目录；
- 已声明文件内容哈希变化；
- Meetily 进程未关闭；
- 正式变更单未明确确认回滚。

## 8. 审计专用未签名开关

`-AuditAllowUnsignedInstaller` 只允许在一次性隔离环境验证未签名候选。它会让报告出现 `signaturesRequired=false`。生产环境、用户机器和正式发布验收严禁使用；若生产报告出现该值，验收必须判定为 FAIL。

## 9. 证据保留

至少保留：

- 所有权清单及 SHA-256；
- 目标／恢复安装器及 SHA-256、签名主体、证书链和时间戳；
- CaptureSnapshot、Preflight、Rollback 三份 JSON；
- 目标兼容快照和紧急快照的清单 SHA-256；
- 回滚前后注册版本、EXE SHA-256 和签名；
- 回滚后启动和关键数据人工验收；
- 失败时的全部失败报告和自动恢复证据。
