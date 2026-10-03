# T03 模板字段识别验收

2026-10-03。起点 9461acacb2fb43d209e1bd592b204cd6f1c9f69d。新增共享 field_schema，清空、追溯、恢复使用同一份中英文精确标签、任务列、行内标签和表格解析。保留原列名和顺序，不批量改写模板；未知 Success Metric 不猜验收含义。表头必须紧跟分隔行，数据里的状态字样不改变列映射；加粗、全角／空格、转义竖线及 fenced 示例有检查。

真实生产模块失败基线10案例：3通过7失败、退出1；修复后加入 review 反例，共11通过0失败。R03-02 自审发现“依赖：测试环境就绪；虚构卡点”会连带恢复未验证尾部，先复现10通过1失败，再修正组合值完整性后11通过。不是删校验来填表。

最终原生 source_binding 28、field_schema 4、meeting_context::summary 48、人工保存5、历史9，共94通过0失败，1项特定2B模型检查忽略。前端依据／source契约／i18n30通过；Next和产品构建退出0。更新旧静态调用名断言时保留生成入口、原文、模板和底层校验调用的断言。

实际电脑能力启动 Windows 候选0.4.3，SHA256 `e5caf61f6a84e11ff926cb66834cb5d711130f448e1bd5258dd1618359a4f091`。中间候选本项目crate临时opt-level=0，T08仍需标准优化Release。桌面 qwen3.5:4b 使用历史快照实际生成 `gen_509de7eea1db49ff80c7935ca12df9f5`，94.81秒。依赖或卡点原列名不变，两值分别保存为测试环境就绪／接口回归测试通过，依据分别是虚构片段2／3，有对应哈希。已实际横向滚动查看最后一列，离开并重新打开，再展开依赖依据核对原文。固定4段虚构输入和合成偏移不能当成真实ASR。

范围：C01同义时间由native和真实生产函数验证；C02组合列识别由桌面生成、重新打开、原文面板及反例证明；C03 Deliverable 和未知任务列由native及生产函数证明，没有宣称额外英文桌面生成；C04/C05格式、重排、行内、未知列与数据行由native和生产函数证明。每个Task已进行桌面实测，不把函数覆盖冒充每种模板都在桌面点过。

后续待修实测问题：模型本次输出ISO截止时间，现有原文值匹配不支持，因此时间待核对（T06）；验收、状态虽已经识别，但仍待T04增加依据检查。数据库保存10个字段候选和3条警告，桌面打开只显示4个受支持字段，待核对候选／警告在读取时消失的问题纳入T07，未当作通过。T03只确认识别和恢复的规则一致。T04、T06、T07及完整Release验收未完成。

Review通过：旧会议metadata逐字节不变；当前录音context与模板引用不变；模板原文件SHA不变；manual入口、历史逻辑未改；没有Cargo、依赖、node_modules改动。共用代码减少重复别名与解析代码；新字段尚无证明时仍需核对。T03必改问题0，以上跨任务问题已有对应未完成Task，必须完成后才可宣布整体修复通过。

本地证据 `_ux_audit/会议摘要未提及修复验收-20261003/T03`：before-business.log／results、after-business.log／results、review-incomplete-combined-before.log、native五组日志、frontend-regression.log、两个构建日志、source-manifest.json、review-boundaries.json、desktop-summary-result.json、desktop-generation-snapshot.json、desktop-generation-history.json、desktop-final-markdown.md、C02系列桌面截图和UIA文字。外部原始会议没有重放。
