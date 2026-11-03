# Mac 中文定制版构建

本文件面向当前 Meetily 定制源码。Windows 安装包不能转换为 Mac 安装包，需由 macOS 编译源码。

## 云端构建

在私有 GitHub 仓库的 Actions 页面选择“Mac 中文定制版安装包”，点击 Run workflow。

- `apple`：Apple 芯片（M 系列），生成 `aarch64` 安装包。
- `intel`：Intel Mac，生成 `x64` 安装包。
- `both`：两种都构建。

构建成功后，在对应运行的 Artifacts 下载压缩包，解压取得 `.dmg`。输出仅保留 14 天，下载后请自行保存。工作流只手动触发，使用标准托管运行器，不修改计费预算；额度不足需由账号所有者处理。

当前构建目标要求 macOS 14.4 或以上，以匹配系统声音采集代码。打开 DMG 后，将应用拖到 Applications（应用程序）。首次录音根据系统提示授予麦克风、系统音频或屏幕录制权限。

本构建使用临时签名，没有 Developer ID 签名及 Apple 公证。不能将构建成功等同于已通过 Gatekeeper；公司分发应由 IT 确认签名部署方式。

## 本机构建

需有 Xcode 开发工具、Node.js 20、pnpm 9.15.9、Rust stable、CMake。在源码根目录运行：

```bash
bash scripts/build-macos-custom.sh
```

脚本按当前 Mac 芯片选择目标，编译纪要辅助程序，再生成 APP 和 DMG。安装包位于 `target/<架构>/release/bundle/dmg/`。

## Lark 凭证与迁移

Mac 版将本机随机加密密钥保存至登录钥匙串，数据库仅保存带认证的 AES-256-GCM 密文。拒绝钥匙串访问时不会退回明文保存。

Windows 的凭证绑定 Windows 用户，不能直接迁移解密到 Mac。复制会议资料不等于完成 Lark 登录；在 Mac 中重新填写所属公司的 App ID / App Secret 并授权。钥匙串丢失时，旧凭证仍会保留，但无法解密，应重新配置授权。

定制构建不生成官方更新签名文件，并清空官方自动更新地址，以免覆盖定制功能。后续更新使用新的定制安装包。

## 验收

安装包产生后仍需在真实 Mac 验证：启动、麦克风与系统声音、中文转写、纪要生成、目录选择、暂停恢复、公司切换、钥匙串重启读取、Lark 授权、群内开会与人工确认发送。云端构建不能替代这些真实设备验收。
