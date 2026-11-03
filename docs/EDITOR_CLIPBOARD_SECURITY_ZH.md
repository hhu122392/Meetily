# 纪要编辑器粘贴漏洞修复

修复日期：2026-11-02。

## 修复内容

针对 `GHSA-c8x8-7fp4-3x9w` / `CVE-2026-104847`，将 `prosemirror-view` 从 1.41.3 固定升级到 1.42.3，并按新版本的依赖要求将 `prosemirror-model` 从 1.25.3 升级到 1.25.8。其他 ProseMirror 组件的固定版本保持原样。

漏洞涉及粘贴 HTML 的 `data-pm-slice` 上下文：旧版可能绕过节点属性校验。修复版在恢复上下文前执行属性校验，拒绝不合法的上下文属性。维护者公告：<https://github.com/ProseMirror/prosemirror-view/security/advisories/GHSA-c8x8-7fp4-3x9w>。

`packageManager` 固定为 pnpm 9.15.9，与现有发布构建使用的版本一致，确保 `package.json` 中的统一版本覆盖规则按预期生效。锁文件同步更新。新增 jsdom 仅用于开发测试，不由业务组件引入。

## 验证

安装依赖时使用锁文件：

```powershell
pnpm install --frozen-lockfile
```

安全与兼容性回归：

```powershell
bun test tests/lib/editor-clipboard-security.test.js
```

以上命令在 `frontend` 目录运行。测试直接调用当前编辑器依赖的粘贴解析器，验证不合法属性被拒绝、合法上下文仍保留、中文和加粗格式正常、表格单元格保留、纯文本粘贴不恢复 HTML 上下文。测试使用不执行脚本、不联网的 DOM 环境。

本次还以 npm 官方 1.41.3 发行代码进行对照：旧版在“不合法上下文属性”用例中失败，1.42.3 通过。该对照验证的是属性校验缺陷，不是对真实用户应用实施脚本或系统命令执行。

Markdown 保存、保存冲突、发送前确认、附件选择与消息预览等相关测试同时通过。正式前端构建及类型检查通过，依赖扫描不再报告这条 ProseMirror 告警；其他依赖的既有告警不在本次修复范围内。

## 生效条件

代码和锁文件修复同时适用于 Windows 与 macOS，但旧安装包不会自行获得修复。必须重新构建对应平台安装包并安装。已有会议资料、转写模型及 Lark 配置不需要迁移。

Windows 和 Mac 安装包、安装状态应分别核对；仅修改源码或生成一个平台的安装包，不表示另一平台或用户已安装程序已经修复。
