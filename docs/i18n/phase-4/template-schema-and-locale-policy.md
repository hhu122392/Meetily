# 阶段 4：模板 Schema、内容 Locale 与兼容策略

## 1. 适用范围

本策略只管理“内置会议总结模板内容”的语言，包括模板名称、描述、章节标题、章节指令、条目格式和示例格式。它不替代 UI Locale、转录语言或摘要语言设置。

## 2. 数据契约

- Schema：`frontend/src-tauri/schemas/template-v2.schema.json`。
- 内置资源：`frontend/src-tauri/templates/en/*.json` 与 `frontend/src-tauri/templates/zh-CN/*.json`。
- 每个资源必须是 Schema V2，并具有稳定 `id`、`locale`、`version`、稳定章节 `id` 和 `source.type = builtin`。
- 同一模板的英文/中文版本必须具有相同模板 ID、版本、章节数量、章节 ID、格式、必填标志和空值行为；仅可本地化用户可读内容。
- 历史拼写 `psychatric_session` 保留为稳定 ID，避免破坏旧偏好、快照和会议引用；显示名称已正常本地化。

## 3. 内容 Locale 选择

内容 Locale 的唯一业务输入是 `summaryLanguage`，UI Locale 不参与摘要生成模板选择：

| `summaryLanguage` | 模板内容 Locale |
|---|---|
| 中文及中文区域变体 | `zh-CN` |
| 英文及英文区域变体 | `en` |
| 其他明确语言 | `en`（当前仅提供英文/简体中文） |
| `__auto__` / 空值 | 已知检测语言时据其解析；生成前尚无检测结果时使用 `en` |

若请求的内置 Locale 资源不存在，解析器必须回退到 `en`，不得泄露文件路径或导致崩溃。

UI 中的模板库和模板选择器可按当前 UI Locale 显示内置模板名称与描述；这只是展示本地化，不写回模板选择、转录语言或摘要语言。

## 4. 数据优先级与保护

同一稳定 ID 的有效来源优先级为：

1. 用户自定义模板；
2. 新版多语言内置模板；
3. 旧版平铺 bundled 模板。

Locale 切换不会重写用户文件。用户自定义模板是单份、用户拥有的内容，不因内容 Locale 被替换。新增、更新、删除、恢复仍沿用冲突检测、备份与回收站机制。

升级时，旧版 `templates/<id>.json` 可继续被 V1 兼容加载器读取；若其 ID 已有新版多语言内置资源，则使用新版内置资源，避免残留英文文件遮蔽中文版本。非内置 ID 的旧 bundled 模板仍可正常读取。

## 5. 历史会议与生成行为

- 已生成会议保存不可变模板快照；再次打开或重生成历史会议时优先使用该快照，不随资源升级或 Locale 切换漂移。
- 未保存快照的新生成请求按 `summaryLanguage` 解析内置模板内容 Locale。
- 自定义模板始终按用户内容生成，不进行隐式机器翻译或覆盖。
- Prompt 的最高语言约束来自 `summaryLanguage`；模板内容只控制结构与业务语义，不得推翻用户明确选择的摘要语言。
- 转录文本被视为数据，不得把其中的指令当作系统指令执行。

## 6. 版本规则

- `id`：永久业务身份；本地化不创建新 ID。
- `version`：模板业务结构或内容发生发布级变更时递增；同次发布的各 Locale 保持一致。
- `locale`：使用规范值 `en` 或 `zh-CN`；用户导入内容允许为空，但内置资源不允许为空。
- `extensions.meetily.content_revision`：用于记录同一 Schema 版本下的内容修订。

## 7. 审计与放行

自动门禁包括 Schema、ID 唯一性、跨 Locale 结构、104 条候选覆盖、中文完整性、回退、语言隔离、自定义模板保护、旧数据兼容和 Prompt 服从性。自动化通过不替代语言质量双人审校；双人签字未完成时，阶段结论最多为 `CONDITIONAL PASS`。
