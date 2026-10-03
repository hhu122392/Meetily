# T02 摘要人员填写验收

2026-10-03。实施起点 c78f2ea6c66ddc4914617c2a39661ed3fd99fe91。生成保存只读取已经解析的模板和本次已确认人员快照，填模板要求的人员字段；人工保存入口保持原样。支持明确行内字段、键值表、人员列和既有段落补缺；无法确定位置时提示待核对。未要求人员的模板不加章节，姓名候选和待确认不当成出席，未指定主持人不猜。

原生真实代码检查：meeting_context::summary:: 46通过0失败1忽略（特定2B模型环境）；summary::commands::tests:: 5通过；summary::template_snapshot::tests:: 9通过，均退出0。60项通过。额外生产生成函数6案例、i18n17检查、Next构建和桌面候选构建退出0。初次工具接入编译错误、一次误传不存在fixture路径、DLL路径缺失不是业务失败，修正环境后重跑通过；日志保留。

桌面候选0.4.3 SHA256 `d8eb5bac40b7cbd3ef6b91dfcea77cdb1ccf50b8051fd56813c98055f3121b8f`。中间功能候选对本项目crate临时opt-level=0，依赖仍按Release构建；最终T08必须标准优化Release重新整体验收。没有修改Cargo.toml或增加依赖。

电脑能力启动实际Windows桌面程序，在虚构会议 `meeting-1bed57b2-bba7-4530-9b8a-aa66093ac7ff` 上用本机既有builtin-ai qwen3.5:4b真实生成。4段固定虚构转录不是真实ASR。已确认林舟、陈岚出席，顾然缺席，主持人为空。生成 `gen_73f6020e06bf4cb987bfe29d1dafcc1f` 使用历史快照，保存正文含“参会人员：林舟、陈岚；缺席人员：顾然；主持人：会议未提及”；Action负责人仍由独立原文依据恢复，不由参会人员自动填入。

首次桌面验收发现补入字段与下一标题合并。R02-01修正段落分隔，增加原生回归后重建、实际重新生成及重新打开，标题已独立显示，名单保存一致；没有只依据函数测试宣布通过。当前验收／状态／组合依赖字段仍待T03—T06修复，截图不当成这些后续任务通过。

N01生产强制空字段／遗漏、桌面实际生成并保存；N02姓名候选和未知主持人由真实生产检查与桌面主持人空验证；N03无人员要求及排除要求由原生检查；N04结构化冲突用已确认快照替换、历史来源9项native检查及桌面历史重生成；N05幂等、普通正文、代码示例与人工保存5项native检查。函数案例和桌面操作范围分开记录。

Review通过：生产4文件；唯一生成入口接入，人工保存未改；字段位置来自生成时的模板；不读取当前全局模板／人员文件；旧会议metadata逐字节不变、当前会议人员上下文和模板引用不变。当前metadata只新增detected_summary_language=zh，这是摘要语言识别保存。模板原文件SHA256不变。无法匹配的字段notice用native固定翻译键重开保留，不采信前端validation。必改问题0。

完整本地证据 `_ux_audit/会议摘要未提及修复验收-20261003/T02`：production-wrapper-results.json、native-tests.log、native-manual-save.log、native-history.log、i18n-regression.log、frontend-build-v043.log、rust-build-v043.log、source-manifest.json、review-boundaries.json、desktop-generation-snapshot.json、desktop-generation-history.json、desktop-summary-result.json、desktop-final-markdown.md、N01/N04桌面截图及文字、N01-reopened截图及核对记录。外部原始会议未重放。
