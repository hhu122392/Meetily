# 阶段 5A-2 Windows 安装、升级、降级、卸载与数据保留报告

- 审计日期：2026-08-23
- 隔离产品：`Meetily Phase 5A2 RC`
- 隔离标识：`com.meetily.ai.phase5a2rc`
- 全新安装链路：**PASS（19/19）**
- 合成升级链路：**PASS（21/21）**
- 真实历史数据迁移门禁：**OPEN**

## 1. 安全隔离

所有安装操作只作用于 `%LOCALAPPDATA%\Meetily Phase 5A2 RC`、`%APPDATA%\com.meetily.ai.phase5a2rc` 和对应 HKCU 卸载注册项。脚本在任何递归清理前验证解析后的绝对路径、预期叶子名和 Reparse Point；应用及安装器均通过隐藏窗口启动。正式 Meetily、用户录音目录和共享模板位置不在操作范围。

## 2. 全新安装／修复／卸载

| 步骤 | 实测结果 | 判定 |
|---|---|---|
| 前置清洁 | 隔离注册项、安装目录、AppData 均不存在 | PASS |
| 安装 | Exit 0；名称、0.4.0 版本和路径正确 | PASS |
| Payload | 主程序存在；helper／FFmpeg 哈希精确匹配构建输入 | PASS |
| 启动 | 5 秒后存活；工作集 63,602,688 字节 | PASS |
| 同版本修复 | Exit 0；数据哨兵哈希不变 | PASS |
| 默认卸载 | Exit 0；注册项与程序目录移除 | PASS |
| 数据策略 | 默认卸载保留应用数据，哨兵哈希不变 | PASS |
| 审计清理 | 隔离注册项、程序目录、AppData 均不存在 | PASS |

## 3. 合成 0.3.9 → 0.4.0 升级／降级阻止

| 步骤 | 实测结果 | 判定 |
|---|---|---|
| 0.3.9 安装 | Exit 0；注册和 Payload 版本均为 0.3.9 | PASS |
| 原位升级 | 0.4.0 安装 Exit 0；注册和 Payload 版本均为 0.4.0 | PASS |
| 数据保留 | 升级前后哨兵 SHA-256 一致 | PASS |
| 升级后启动 | 5 秒后存活；工作集 52,580,352 字节 | PASS |
| Sidecar | helper／FFmpeg 哈希与 0.4.0 构建输入一致 | PASS |
| 静默降级 | 0.3.9 安装器 Exit 3，符合 NSIS hook 约定 | PASS |
| 降级无副作用 | 注册仍为 0.4.0，Payload EXE 和数据哨兵哈希不变 | PASS |
| 升级版卸载 | Exit 0；程序清理，数据按默认策略保留 | PASS |
| 最终清理 | 所有隔离状态均清理 | PASS |

## 4. 产物对应关系

| 产物 | 字节 | SHA-256 | 签名 |
|---|---:|---|---|
| 0.4.0 RC EXE | 65,776,128 | `D4C8E31D9504B25DC8C77DC30C0C5A506055E2C20BE082347B763901AA640D84` | NotSigned |
| 0.4.0 RC NSIS | 43,996,335 | `5CFE9C41360EC135C407FF8FC8D00EE23BAE46CB2E1E8632DAC51EE91BA61FBE` | NotSigned |
| 0.4.0 安装后 Payload | 65,776,128 | `72A46EECB9F8E18FEB19ACCE315E02C97BC757B98EE7A74E4C45287B674BD996` | NotSigned |
| 合成 0.3.9 NSIS | 43,979,042 | `939A6DD29983C453CBC62A9F44C0D61CC3ADB7C3D956A15A60414311C6FCBADC` | NotSigned |
| `llama-helper` | 3,773,440 | `EE4E303B64603DFF8369644EDF3792ADD1948605C62F3BEC66A2549757EF97EC` | NotSigned |
| FFmpeg | 99,264,000 | `5AF82A0D4FE2B9EAE211B967332EA97EDFC51C6B328CA35B827E73EAC560DC0D` | NotSigned |

安装后 Payload 与打包前 EXE 哈希不同是 NSIS 打包阶段写入 Bundle 类型信息的预期行为；两者版本、大小和各自哈希均单独记录，不能互相替代。

## 5. 正式产物保护

- `target/release/meetily.exe`：`1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823`；
- `target/release/bundle/nsis/meetily_0.4.0_x64-setup.exe`：`C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434`；
- 两项在两轮系统审计前后均不变。

## 6. 未关闭范围

合成旧版只用于验证安装器版本比较、原位替换、数据目录保留和卸载语义。以下仍未覆盖：

- 使用真实已发布历史版本及其批准安装包；
- 携带脱敏的旧会议数据库、录音、模型、旧设置和自定义模板完成升级后的内容可读性检查；
- 使用批准的回滚包验证旧版本能读取候选版产生的数据；
- 交互式“删除应用数据”UI；
- 已签名生产包的 UAC、SmartScreen、时间戳和证书链。

所以 `P5-UPG-001` 与 `P5-SIGN-001` 继续 OPEN。
