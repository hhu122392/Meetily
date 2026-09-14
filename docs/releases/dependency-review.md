# 0.4.2 依赖复核

2026-09-14，Windows 桌面发行范围。

更新了 linkify-it、markdown-it、nanoid、PostCSS 等兼容补丁，并锁定 pnpm 10.34.1。`pnpm audit --prod` 的告警从 44 项减少到 25 项；不能把该结果写成“零漏洞”。

剩余告警按用途处理：

- Next.js 14.2.35：23 项版本告警。本安装包使用 `output: 'export'` 的静态页面，不携带 Node.js，也不启动 Next.js 服务端、图片优化接口或开发服务器。源码里的 `next dev` / `next start` 不应作为公网服务部署；服务端升级不属于本次桌面发行。
- `@tiptap/core` 2.27.2：针对 [GHSA-cp6q-959q-f8rh](https://github.com/advisories/GHSA-cp6q-959q-f8rh)，按官方建议在合并属性前过滤 `__proto__`。补丁同时覆盖源文件、ESM 和 CommonJS，通过 pnpm 的 `patchedDependencies` 随锁文件应用。新增测试复现修补前的失败，并检查修补后原型、继承属性及正常样式合并。版本扫描仍会报告此项；没有忽略审计规则。
- `uuid` 8.3.2：告警涉及 v3/v5/v6 的调用方缓冲区参数。当前 BlockNote 的 UUID 调用使用 v4；本次未升级编辑器整套主版本。以后改变调用方式或升级编辑器时需重新审查。

此记录只解释本次检查发现和桌面包的使用范围，不表示完成了全部第三方代码安全审计。
