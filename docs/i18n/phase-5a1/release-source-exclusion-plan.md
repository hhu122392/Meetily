# Meetily 5A-1 发布源码排除清单

日期：2026-08-23

## 1. 已通过 `.gitignore` 隔离的本地产物

5A-1 启动时共有 216,215 个未跟踪文件，其中 215,373 个属于隔离构建、恢复缓存、本机工具或审计压缩包。新增以下根目录规则后，这些文件不再进入 Git 发布源码视图：

```gitignore
/target-phase*/
/target-tools/
/target-phase*.zip
/target-phase*.zip.sha256
/frontend/.next.corrupt-*/
```

规则只改变 Git 候选视图，没有删除任何文件。

排除范围包括：

- `target-phase1-*` 至 `target-phase5-*` 的 Cargo/Tauri 构建结果；
- 隔离 EXE、NSIS/MSI 包和 VM 审计包；
- `target-tools` 下的本地 libclang、Ninja 和其他构建工具；
- `.next.corrupt-*` 恢复目录；
- 各阶段临时构建缓存。

## 2. 生成审计证据

660 个历史生成证据文件归类为 `EVIDENCE_ARCHIVE_EXCLUDE_SOURCE`，约 49 MB。它们包括：

- CDP/运行时诊断 JSON；
- 页面和控件快照；
- PNG 截图；
- 安装、运行、完整性和回归原始证据；
- 包含本机绝对路径的历史清单。

这些文件应保留在不可变审计归档和 `target/release/docs` 交付目录中，但不进入生产源码提交。否则会造成仓库膨胀，并把本机路径和大量可再生成证据混入源码历史。

`phase-5a1-source` 下的四份紧凑审计产物是例外：`phase5a1-audit-report.md` 和 `verification-summary.json` 进入机器清单；`source-inventory.json` 与 `sensitive-content-audit.json` 为避免自递归不纳入它们自己的逐文件清单。四份文件均进入发布源码文档提交，使克隆后的纳入／排除边界可独立复核。

## 3. 不纳入的元数据差异

| 文件 | 处置 | 原因 |
|---|---|---|
| `frontend/src-tauri/.cargo/config.toml` | `EXCLUDE_LINE_ENDING_ONLY` | 只有文件末尾换行差异，与产品行为无关 |
| `frontend/next.config.js` | `EXCLUDE_STAT_ONLY` | Git 状态显示变化，但无内容 Diff |

不得为了“让工作树看起来整齐”而把这两项伪装成产品修改提交。

## 4. 敏感信息与本机路径

敏感审计覆盖 682 个文本文件、约 30.7 MB：

- 强特征私钥、OpenAI 风格密钥、GitHub Token、AWS Access Key、Slack Token：0；
- 13 处凭据样式字符串：全部为语言文案、测试夹具或审计夹具；需要人工判别的剩余项：0；
- 绝对路径文件：10 个；
- 其中 8 个属于独立证据归档，2 个属于 Windows/POSIX 路径解析测试夹具；
- 仍需在公共提交前清洗的版本化文档路径：0。

`docs/i18n/README.md` 原有的当前工作区绝对路径已改为可移植的 `$repoRoot` 示例。

## 5. 严格禁止项

以下内容不得进入发布源码提交：

- 任何 `.env`、API Key、模型密钥或用户配置；
- 真实会议数据库、录音、逐字稿或模型缓存；
- 未签名候选 EXE／安装包；
- `target-phase*` 编译目录；
- `target-tools` 本机构建工具；
- 历史 `.next.corrupt-*`；
- 生成运行证据；
- 仅换行／状态缓存造成的伪修改；
- 未经审核的安全配置和升级身份变化。
