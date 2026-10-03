## T07：前端依据和兼容回归

实施记录：2026-10-04完成。144原生检查通过0失败，1项原有特定模型检查忽略；10生产案例和T04/T05/T06共129回归通过；51轻量源码检查、55前端i18n、6依据回归及新增刷新回归组11通过。最终候选0.4.3 SHA256 2bdb9df245dc2d1f367a5067ad7fde32a38e8d7fedd68293824986a11c817955，本项目crate中间opt-level=0；标准Release待T08。桌面历史快照生成gen_584eeada58ec4d7d9939c374929fc08b，qwen3.5:4b耗时89.47秒。

| 案例 ID | 验收内容与预期 | 实际／证据 | 状态 |
| --- | --- | --- | --- |
| H01 | 新字段和对应原文、待核对提示可读；缺键fallback | final-source-en-open/final-related-en：旧真实结果10候选、8待核对，原值保留，相关线索不当证明；final-fields-zh/final-acceptance-proof-zh：中文字段和真实QA片段；实际资源fallback检查通过 | 通过 |
| H02 | 旧摘要可读；expected不自动当实际出席 | final-stored-loaded：旧模型结果；legacy-json-loaded：独立虚构BlockNote JSON正常打开，fixture-manifest明确非AI生成；native-context/history覆盖旧expected和快照读取 | 通过 |
| H03 | 人工保存、重启正文不丢；提示与正文分开 | manual-edited/manual-saved/manual-after-restart及manual-restart-check：T07人工待确认正文SHA一致，原生成链接保留；原生伪造WebView候选反例拒绝 | 通过 |
| H04 | 原文修改使旧证据失效；重生成沿用所选快照 | 实测恢复旧人工版本、原文0改1，旧引用不可用；最终refresh-one/zero-body-retained正文保持恢复版本，final-historical-snapshot/retry与原生结果证明historical_snapshot；新生成10字段supported、0警告 | 通过 |
| H05 | 旧处理缓存拒绝、新有效可复用；旧正文保留 | native-cache25通过：版本2026091306拒绝、2026100401可复用；真实桌面新结果缓存版本2026100401；旧正文及人工历史保留。缓存命中反例属于原生检查，未冒充额外桌面命中 | 通过 |

Review检查：

- [x] 前后端字段类型兼容；复用已存在字段及语言键，只补齐说明和两项警告映射。
- [x] 只从native存储保留旧待核对候选，核对槽位含义和任务；永不升级为证明，前端人工保存不接受伪造校验。
- [x] 原文版本变化清除旧关联引用；读取依据所记录的真实快照，人工保存按当前上下文核对，正文保持。
- [x] 缓存版本条件明确，没有批量回写历史；虚构QA之外摘要、历史、人工版本未改。

Review记录：基线4例3失败，候选槽位边界7例3失败，原生英语缺席句1失败，均保留失败记录并修正。桌面发现原文refetch清空metadata并触发初始loader，使PageContent卸载，恢复后的正文又显示进入页时的旧内容；数据库正文未丢。新增实际hook检查先失败后通过，保留加载页并刷新数据，最终程序桌面0→1→0复测正文没有回退。api_get_summary只重算返回值、不写数据库；故刷新后DB保留最近一次restore校验，不能用该旧JSON冒充当前返回证据。最终桌面新生成10字段与原生结果一致，人员正文误报消除，真实冲突反例仍警告。必改问题0，T07验收review通过后单独推送；提交号见push-result.json。合成片段偏移不是实际ASR，另一电脑原会议未重放。两个虚构QA的受控旧结果/JSON注入有单独备份和说明，不算模型生成。T09/T08仍待完成。

证据目录：C:/Users/zhoua/Desktop/Meetily-程序员交接-20260908/_ux_audit/会议摘要未提及修复验收-20261003/T07

最终构建日志：refresh-final/frontend-build-v043.log、refresh-final/rust-build-v043.log。失败及中止日志不算通过。
