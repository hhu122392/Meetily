# Meetily 0.4.2 验收记录

日期：2026-09-14。环境：Windows 11 x64，系统版本 10.0.26200。

## 源码及自动化检查

- 生产源码提交：`8dd43e691007eb86f3a65f028acadbaf5dc15737`。
- 前端 281 项测试通过；TypeScript 类型检查、ESLint 检查通过；Next.js 静态页面构建通过。
- Rust 常规测试 822 项通过：应用库 788、摘要事实约束 15、摘要持久化 12、模板排版 7。应用库默认忽略 23 项需要外部材料、模型或设备的测试。
- 其中两项私有录音测试另行执行并通过：尾段不包含补零、实时分段样本与源时间位置完全对应。合计 824 项 Rust 测试通过；其余 21 项外部条件测试不算通过。
- 大量测试并行执行时，辅助进程强制退出测试曾出现一次超时。随后串行全套通过，该用例又独立连续执行三次通过，每次确认 Job 中剩余进程为零。保留原失败记录，没有修改退出测试断言或生产超时阈值。
- 数据库中 18 条已应用迁移的 SHA-384 与本次源文件完全相同。Git 属性固定迁移的原始字节，防止另一台电脑的换行转换改变 SQLx 校验值。
- 源码清单检查排除了录音、会议数据库、密钥、开发缓存和旧程序备份；凭据规则扫描没有命中。第三方依赖的版本告警和补丁见 [依赖复核](dependency-review.md)，并非零漏洞声明。

本地外部材料测试需要设置 `MEETILY_QA_FIXTURE_DIR`，目录中包含 `MAIN-013-A/B/C-20s-input.json` 及 `MAIN-007-客户端验收/fixed-zh.wav`。录音不随仓库分发：

```powershell
$env:RUST_TEST_THREADS = "1"
cargo test --locked -p meetily --test app_lib_tests --test summary_fact_grounding --test summary_persistence --test summary_template_layout
$env:MEETILY_QA_FIXTURE_DIR = "C:\your-local-acceptance-fixtures"
cargo test --locked -p meetily --test app_lib_tests audio::vad::tests:: -- --ignored
```

## 安装包验收

- NSIS 安装包 395,549,337 字节，7-Zip 完整性测试和解包通过。
- 13 个必需程序/运行库均存在且非空，包括主程序、FFmpeg、摘要助手、MOSS 助手、VC++、ONNX、sherpa 和 DirectML；包含固定版 WebView2 及第三方许可文件。
- 静态 DLL 依赖检查通过；依赖解析到安装包自身或 Windows 系统目录。
- 限制 DLL 搜索范围后，包内 sherpa 运行库成功加载并报告 1.13.7；将 PATH 限制为系统目录后，FFmpeg 与摘要助手启动检查通过。
- 主程序无参数的测试入口按源码约定退出 2；完整参数的独立工作线程生成了预期的受控失败记录。这验证程序能加载及执行工作线程，不等于转写准确率测试。
- 本次安装后的界面验收未完成，不能写成已完成全新电脑安装验收。没有停止现有客户端，也没有更改或删除用户会议数据。
- 安装包无 Authenticode 发布者签名；模型需首次使用时下载。发布页提供 SHA-256 校验文件。

## 范围

本地摘要的事实关系质量仍有已知限制。自动化检查不能替代真实会议内容质量验收，也不能保证每台电脑的麦克风、显卡或模型下载网络情况。未将未执行的外部模型测试、全新 Windows 虚拟机测试或其他操作系统测试记为通过。
