# 阶段 5 Windows 安装、修复、卸载与数据保留报告

审计日期：2026-08-23
测试产品：`Meetily Phase 5 Audit 0.4.0`
隔离标识：`com.meetily.ai.phase5audit`
已执行链路结论：`PASS`
正式覆盖升级／版本回滚：`OPEN`

## 1. 隔离原则

候选包使用独立产品名、独立 Bundle ID 和独立安装目录；没有覆盖 `target/release/meetily.exe`，也没有安装或卸载正式 Meetily。测试结束后，隔离注册项、安装目录和隔离 AppData 均已清理。

## 2. 实测链路

| 步骤 | 预期 | 实际 | 结果 |
|---|---|---|---|
| 清洁前置 | 无隔离注册项、安装目录和 AppData | 三项均不存在 | PASS |
| 静默全新安装 | 返回 0 | 0 | PASS |
| 注册表 | 名称、版本、路径正确 | `Meetily Phase 5 Audit`、0.4.0、独立 LocalAppData 路径 | PASS |
| 安装后启动 | 5 秒后仍存活 | 存活，工作集 55,111,680 字节 | PASS |
| 测试数据夹具 | 在独立 AppData 写入 Sentinel | SHA-256 已记录 | PASS |
| 同版本修复 | 返回 0，数据不变 | 返回 0，前后 Sentinel 哈希一致 | PASS |
| 默认静默卸载 | 返回 0 | 0 | PASS |
| 程序清理 | 注册项和安装目录移除 | 均已移除 | PASS |
| 默认数据策略 | AppData 保留 | Sentinel 与哈希保留 | PASS |
| 审计后清理 | 只删除隔离测试 AppData | 隔离目录已删除；用户录音／共享模板未触碰 | PASS |

## 3. 产物对应关系

| 产物 | 字节 | SHA-256 | 签名 |
|---|---:|---|---|
| 隔离候选 EXE | 65,631,232 | `A58EAB4B3076D520A732197AD4CDBE5EEB8732C9A85433509A94FB097B75D94B` | NotSigned |
| 隔离 NSIS 安装包 | 43,979,110 | `71777BAA1EC9F0DBAD2643638B884DC3DF6706AFFDF61EBF9CECFBED133A4101` | NotSigned |
| 安装后的 NSIS Payload EXE | 65,631,232 | `56D059E3CF5CC2FEE51697E7DD3FC002FF61648D3CDBEC803E075BF4FEF55337` | NotSigned |

安装后 Payload 与未打包候选 EXE 的大小相同、哈希不同，是 Tauri NSIS 打包器把内部 Bundle 类型占位符从 `UNK` 改为 `NSS` 所致；逐字节诊断只有这 3 个字节发生变化。机器可读证据同时保存两侧哈希，不能拿未打包 EXE 哈希替代已安装 Payload 哈希。

## 4. 正式产物保护

- 正式 EXE SHA-256：`1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823`；
- 正式安装包 SHA-256：`C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434`；
- 安装测试前后两项均不变。

## 5. 尚未覆盖

- 从用户正在使用的正式 0.4.0 原位覆盖升级到中文候选；
- 携带旧会议数据库、模型、录音目录、自定义模板和旧设置的升级；
- 使用批准的回滚包从候选版降回上一批准版本；
- 交互式卸载中“删除应用数据”二次确认的人工 UI 操作；
- 已签名生产安装包的 SmartScreen、UAC、安装语言选择和签名链验证。

因此本报告只批准隔离 NSIS 的新装／修复／默认卸载子链路，不批准正式覆盖升级或版本回滚门禁。

## 6. 5A-2 补充执行记录

5A-2 使用干净提交构建的新隔离身份 `Meetily Phase 5A2 RC` 完成 19/19 全新安装链路，并使用同一身份的合成 0.3.9 夹具完成 21/21 升级／降级阻止链路：0.3.9 原位升级到 0.4.0 返回 0，数据哨兵哈希不变，升级后应用可启动；再次运行 0.3.9 静默安装器返回 3，0.4.0 注册版本、Payload EXE 和数据均未改变。

详细证据已转移到 `docs/i18n/phase-5a2/windows-install-upgrade-uninstall-report.md`。该结果批准安装器语义，但未使用真实历史数据和批准回滚包，故真实覆盖升级／版本回滚门禁仍为 OPEN。
