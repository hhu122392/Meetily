# 阶段 5 平台测试矩阵

状态说明：`PASS` 表示有可追溯证据；`OPEN` 表示尚未执行或缺少人工／设备证据；`N/A` 必须由发布范围批准人签字后才生效。

| 平台 | 构建 | 启动 | Locale 热切换 | 托盘 | 通知 | 录音保存 | 安装升级卸载 | 当前结论 |
|---|---|---|---|---|---|---|---|---|
| Windows 11 x64 | PASS | PASS：构建目录与安装后程序均启动 | PASS：50 次，状态隔离通过 | 阶段 3 自动逻辑 PASS，Shell 截图 OPEN | 阶段 3 自动逻辑 PASS，系统截图 OPEN | 硬件／60 分钟长时测试 OPEN | 新装、同版本修复、默认卸载和数据保留 PASS；真实旧版覆盖升级与版本回滚 OPEN | **FAIL／未满足完整门禁** |
| macOS Intel | 未在本机执行 | OPEN | OPEN | OPEN | OPEN | OPEN | OPEN | OPEN／待发布范围批准 |
| macOS Apple Silicon | 未在本机执行 | OPEN | OPEN | OPEN | OPEN | OPEN | OPEN | OPEN／待发布范围批准 |
| Linux AppImage/deb | 未在本机执行 | OPEN | OPEN | OPEN | OPEN | OPEN | OPEN | OPEN／待发布范围批准 |

正式发布若只支持 Windows，项目负责人必须书面批准 macOS/Linux 为本次 `N/A`；否则这些平台仍是发布阻断项。

## Windows 证据范围

- Windows 11 x64 隔离候选构建、运行时截图、离线模式、50 次 Locale 切换和 NSIS 安装链路已有机器可读证据。
- 缩放证据使用 WebView2 `deviceScaleFactor` 模拟 100%、125%、150%、200%；尚未替代 Windows 系统 DPI、实体多显示器和辅助技术人工测试。
- 托盘和通知已有阶段 3 逻辑测试，但没有本轮 Windows Shell 的中英文截图。
- 录音设备枚举由 Rust 集成测试覆盖；真实麦克风／系统音频的 60 分钟录音、暂停、继续、停止、保存没有执行。
- 因隔离候选使用独立产品标识，已验证同版本修复；从正式 0.4.0 原位升级、经批准的降级包和数据回滚仍需专门执行。
