# Meetily 多语言架构与中文化实施方案

## 1. 目标与结论

当前 Meetily 已经支持多语言转录、语言自动检测和多语言摘要，但这些属于业务数据语言，不等于应用界面国际化。当前 UI 仍以英文硬编码为主，没有统一翻译函数、语言资源、Locale Provider、回退规则或原生层语言同步机制。

本方案的目标是先建立英文基线，再加入简体中文，并保证以后扩展繁体中文、日语、韩语及 RTL 语言时不需要再次重构。

首期建议支持：

- `en`：英文，默认和最终回退语言；
- `zh-CN`：简体中文；
- `system`：跟随系统，仅作为用户设置选项，不作为实际 Locale 值。

## 2. 本次文本审计结果

语法树扫描覆盖：

- `frontend/src` 下的 TypeScript、TSX、JavaScript 和 JSX 文件；
- `frontend/src-tauri/src` 下的 Rust 文件；
- Tauri 打包的内置会议模板 JSON；
- JSX 文本、Toast、对话框属性、占位文本、可访问性标签、用户消息属性和 Rust 错误/菜单/通知候选。

去重合并后的英文目录共有 `1354` 条：

| 状态 | 数量 | 处理方式 |
|---|---:|---|
| `translate` | 696 | 已确认的前端 UI 翻译候选，已进入 `en.json` |
| `manual_review` | 62 | 前端示例、内部名称、间接错误或状态消息，迁移时复核 |
| `manual_visibility_review` | 447 | Rust/Tauri 错误、通知或菜单候选，先确认是否会到达用户界面 |
| `excluded_legacy_source` | 45 | 明确来自 `_old` / `old_complex` 等旧代码，仅保留审计记录 |
| `separate_content_localization` | 104 | 内置模板和 AI 提示内容，必须与 UI 语言分开版本化 |

原始出现位置共有 `1549` 个；同一句文本在多个位置出现时，详细目录会保留全部来源。

需要优先迁移的高密度文件包括：

- `frontend/src/components/ModelSettingsModal.tsx`
- `frontend/src/components/AnalyticsDataModal.tsx`
- `frontend/src/hooks/meeting-details/useSummaryGeneration.ts`
- `frontend/src/components/MeetingDetails/SummaryGeneratorButtonGroup.tsx`
- `frontend/src/components/Sidebar/index.tsx`
- `frontend/src/components/About.tsx`
- `frontend/src/components/TranscriptRecovery/TranscriptRecovery.tsx`
- `frontend/src/components/RecordingSettings.tsx`
- `frontend/src/components/WhisperModelManager.tsx`
- `frontend/src/components/ImportAudio/ImportAudioDialog.tsx`
- `frontend/src/components/onboarding/steps/*`

## 3. 必须分开的三类“语言”

多语言重构中最重要的约束，是不要复用当前的 `selectedLanguage` 作为 UI Locale。

建议使用清晰的数据模型：

```ts
type UiLocale = 'en' | 'zh-CN';

interface LanguagePreferences {
  uiLocale: UiLocale | 'system';
  transcriptionLanguage: string;
  summaryLanguage: string | '__auto__';
}
```

三者含义：

| 字段 | 控制内容 | 示例 |
|---|---|---|
| `uiLocale` | 菜单、按钮、Toast、设置、日期和原生托盘 | `zh-CN` |
| `transcriptionLanguage` | Whisper/Parakeet 如何处理音频 | `ja`、`auto`、`auto-translate` |
| `summaryLanguage` | AI 摘要输出语言 | `zh`、`en`、`__auto__` |

一个完全合理的组合是：中文 UI、日语转录、英文摘要。切换界面语言绝不能修改转录或摘要设置。

当前 `ConfigContext.tsx` 使用 `primaryLanguage` 保存转录语言，并同步到 Rust 的 `set_language_preference`。该行为应保留，但字段和 UI 文案应改名为 `transcriptionLanguage`，避免未来维护者误用。

## 4. 技术选型

### 4.1 推荐：`i18next` + `react-i18next`

推荐安装：

```text
i18next
react-i18next
```

原因：

- Meetily 是 Tauri 桌面应用，Next.js 使用 `output: 'export'`；
- 桌面应用不需要 `/en/...`、`/zh-CN/...` URL 路由；
- 需要在运行时立即切换语言，不应重新加载不同路由；
- `react-i18next` 可在客户端 Provider 中工作，适合静态导出；
- 支持命名空间、插值、复数、回退、缺失键回调和类型扩展；
- 可在 React 之外通过同一个 i18n 实例翻译 Hook、服务层和 Toast。

不建议首期采用 Next.js 路由型 Locale 方案。它更适合网页 SSR/SSG 的语言路由，会给 Tauri 静态导出、深链和客户端状态增加不必要的复杂度。

### 4.2 不引入自动浏览器探测插件

Locale 解析逻辑建议由项目自己控制，顺序固定为：

```text
用户已保存的 uiLocale
        ↓ 若为 system 或不存在
navigator.languages / navigator.language
        ↓ 规范化与匹配
支持的 Locale
        ↓ 无匹配
en
```

这样可以避免浏览器插件读取 Cookie、URL 或其他不适用于桌面应用的来源。

## 5. 推荐目录结构

扫描生成的 `en.json` 是迁移基线。实际落地时建议按功能域拆分，降低冲突并便于分批翻译：

```text
frontend/src/i18n/
├── index.ts
├── I18nProvider.tsx
├── locale.ts
├── types.ts
├── formatters.ts
├── generated.d.ts
└── locales/
    ├── en/
    │   ├── common.json
    │   ├── navigation.json
    │   ├── onboarding.json
    │   ├── recording.json
    │   ├── transcription.json
    │   ├── summary.json
    │   ├── models.json
    │   ├── meetings.json
    │   ├── import.json
    │   ├── settings.json
    │   ├── analytics.json
    │   ├── updates.json
    │   ├── errors.json
    │   └── accessibility.json
    └── zh-CN/
        └── 同名文件
```

命名空间建议：

| Namespace | 内容 |
|---|---|
| `common` | 通用动作、状态、单位、确认与取消 |
| `navigation` | 侧栏、页面标题、返回与搜索 |
| `onboarding` | 首次启动流程、权限、模型下载 |
| `recording` | 开始、暂停、继续、停止、音频设备 |
| `transcription` | 转录模型、语言、置信度、增强转录 |
| `summary` | 摘要生成、模板、语言、保存与复制 |
| `models` | 内置模型、Ollama、API Provider 和下载状态 |
| `meetings` | 会议列表、详情、标题、恢复和删除 |
| `import` | 音频导入和重转录 |
| `settings` | 偏好、路径、Beta、About |
| `analytics` | 分析授权和隐私说明 |
| `updates` | 更新检查、版本与下载 |
| `errors` | 基于稳定错误代码的用户错误信息 |
| `accessibility` | `aria-label`、屏幕阅读器专用文本 |

## 6. 翻译键设计规则

### 6.1 使用语义键，不使用英文原文作为键

推荐：

```json
{
  "recording": {
    "actions": {
      "start": "Start recording",
      "stop": "Stop recording"
    }
  }
}
```

不推荐：

```json
{
  "Start recording": "Start recording"
}
```

语义键不会因为英文润色而改变，也便于发现上下文差异。

### 6.2 同义动作优先复用，语境不同则拆分

可以复用：

- `common.actions.save`
- `common.actions.cancel`
- `common.actions.copy`
- `common.actions.delete`
- `common.status.loading`
- `common.status.failed`

不要强行复用：

- 动词 “Record” 与名词 “Recording”；
- “Stop recording” 与 “Stop summary generation”；
- “Model” 在语音识别和摘要 Provider 中的含义；
- “Language” 在 UI、转录和摘要中的含义。

### 6.3 键名使用 lowerCamelCase

```text
recording.errors.modelStillDownloading
summary.actions.regenerate
settings.storage.recordingsLocation
onboarding.permissions.microphone.description
```

### 6.4 产品名和第三方品牌默认不翻译

保留：

- Meetily
- Whisper
- Parakeet
- Ollama
- OpenAI
- OpenRouter
- Claude
- Groq
- Deepgram
- ElevenLabs
- FFmpeg
- BlackHole

模型名称、API 字段、URL、文件扩展名和键盘快捷键也不翻译。

## 7. 插值、复数和富文本

### 7.1 占位符

英文基线中有 50 余条动态文本，部分仍包含源代码表达式。迁移时必须转换为稳定、简单的占位符。

源代码：

```tsx
`Import complete! ${result.segments_count} segments created.`
```

语言包：

```json
{
  "importComplete": "Import complete! {{count}} segments created."
}
```

调用：

```tsx
t('import:status.importComplete', { count: result.segments_count })
```

禁止把以下表达式直接放入语言包：

```text
{result.segments_count}
{labelForCode(pinned)}
{error instanceof Error ? error.message : String(error)}
```

### 7.2 复数

所有数量文案使用 i18next 复数规则，不通过字符串拼接实现：

```json
{
  "segmentsCreated_one": "{{count}} segment created",
  "segmentsCreated_other": "{{count}} segments created"
}
```

简体中文可以让两个键使用同一句，但仍应保持键集合一致。

### 7.3 富文本

包含链接、加粗、按钮或品牌名的句子使用 `<Trans>`，不要拆成不可重排的多个字符串：

```tsx
<Trans i18nKey="analytics:privacy.noMeetingContent">
  No <strong>meeting content</strong> is collected.
</Trans>
```

拆分字符串会导致中文语序和标点难以调整。

## 8. 前端运行时架构

推荐数据流：

```text
系统语言 / 已保存设置
          │
          ▼
    resolveUiLocale()
          │
          ▼
     i18next 实例 ───────► React I18nProvider ───────► 页面、组件、Toast、Hooks
          │
          ├──────────────► document.documentElement.lang / dir
          │
          └──────────────► invoke('set_ui_locale') ──► Tauri 托盘和原生通知
```

建议 API：

```ts
export const SUPPORTED_UI_LOCALES = ['en', 'zh-CN'] as const;
export type SupportedUiLocale = typeof SUPPORTED_UI_LOCALES[number];

export function resolveUiLocale(
  preference: SupportedUiLocale | 'system' | null,
  systemLocales: readonly string[],
): SupportedUiLocale;

export async function changeUiLocale(locale: SupportedUiLocale | 'system'): Promise<void>;
```

保存键建议使用：

```text
meetily.uiLocale
```

不要复用当前转录语言键：

```text
primaryLanguage
```

为了避免首次渲染闪烁，初始化 Provider 前同步读取 `localStorage`；`system` 模式下再读取 `navigator.languages`。切换后同步修改：

```ts
document.documentElement.lang = resolvedLocale;
document.documentElement.dir = resolvedLocale.startsWith('ar') ? 'rtl' : 'ltr';
```

## 9. 日期、时间、数字和语言名称

不要只翻译固定字符串，还必须本地化动态格式。

### 9.1 日期和时间

优先使用浏览器内置 `Intl`：

```ts
new Intl.DateTimeFormat(uiLocale, {
  dateStyle: 'medium',
  timeStyle: 'short',
}).format(date)
```

相对时间使用：

```ts
new Intl.RelativeTimeFormat(uiLocale, { numeric: 'auto' })
```

若继续使用 `date-fns`，必须根据 `uiLocale` 动态注入 `enUS` 或 `zhCN`，不能固定英文 Locale。

### 9.2 数字和百分比

使用 `Intl.NumberFormat`，不要手写逗号、百分号位置或小数格式。

### 9.3 语言名称

当前 `LANGUAGES` 和 `LANGUAGE_OPTIONS` 直接保存 `English`、`Chinese` 等英文名称。推荐存语言代码，然后通过 `Intl.DisplayNames` 显示本地化名称：

```ts
new Intl.DisplayNames([uiLocale], { type: 'language' }).of(code)
```

对 `auto`、`auto-translate`、`zh-tw` 等特殊选项使用语言包键。这样中文 UI 会显示“英语”“中文”“日语”，而英文 UI 仍显示 “English”“Chinese”“Japanese”。

## 10. 字体与布局

当前根布局只声明 `Source Sans 3` 的 `latin` 子集。中文界面至少需要完善回退栈：

```css
font-family:
  "Source Sans 3",
  "Microsoft YaHei UI",
  "Microsoft YaHei",
  "PingFang SC",
  "Noto Sans CJK SC",
  system-ui,
  sans-serif;
```

如果可以接受增加安装包体积，可打包 Noto Sans SC；否则使用系统中文字体回退。

界面测试必须覆盖：

- 中文按钮通常比英文更短，但说明文字的行高不同；
- 标题、下拉框、Toast 和窄侧栏中的截断；
- 中英文混排及模型名称；
- 标点不可出现在行首；
- 将来支持阿拉伯语、希伯来语时的 `dir="rtl"`。

## 11. Tauri 原生层国际化

### 11.1 托盘菜单

`frontend/src-tauri/src/tray.rs` 中的以下文本由 Rust 创建，React 翻译不会影响它们：

- Start Recording
- Pause Recording
- Resume Recording
- Stop Recording
- Open Main Window
- Settings
- Check for Updates
- Quit
- Downloading transcription model
- Starting / Pausing / Resuming / Stopping

建议在 Rust 中嵌入最小原生语言包：

```text
frontend/src-tauri/locales/en/native.json
frontend/src-tauri/locales/zh-CN/native.json
```

Rust 提供：

```rust
set_ui_locale(locale: String)
get_ui_locale() -> String
```

当 Locale 改变时重建托盘菜单。托盘状态变化时使用当前 Locale 获取对应状态文本，不要把翻译后的文字作为菜单 ID；菜单 ID 继续使用稳定英文代码，例如 `toggle_recording`。

### 11.2 系统通知

通知层推荐接收稳定键和参数：

```rust
show_notification("recording.started", params)
```

如果通知始终由前端触发，也可由前端完成翻译后把最终标题和正文传给 Rust。对于后台状态和托盘触发的通知，Rust 必须能独立翻译。

### 11.3 错误处理

不要逐个翻译 Rust 的自由文本错误。推荐将用户可见错误改成结构化错误：

```json
{
  "code": "TRANSCRIPTION_MODEL_DOWNLOADING",
  "params": {
    "model": "parakeet-tdt-0.6b-v3-int8"
  },
  "debugMessage": "..."
}
```

前端映射：

```ts
t(`errors:${error.code}`, error.params)
```

必须区分：

- `code`：稳定、可测试、可用于翻译；
- `params`：可显示的结构化变量；
- `debugMessage`：仅写日志，不直接展示；
- `cause`：底层技术错误，可用于诊断。

本次目录将 447 条 Rust 文本标记为 `manual_visibility_review`，原因是其中一部分只进入日志或内部 Result，另一部分会被前端直接显示。迁移时要逐条沿调用链确认，不应全部放进运行时语言包。

## 12. 内置会议模板和 AI Prompt

模板本地化与 UI Locale 必须分离。用户使用中文界面，并不一定希望模板或摘要自动变成中文。

推荐模板模型：

```json
{
  "id": "daily_standup",
  "locale": "zh-CN",
  "version": 1,
  "name": "每日站会",
  "description": "适用于每日进展、阻塞项和下一步行动。",
  "sections": [],
  "prompt": "..."
}
```

文件结构：

```text
frontend/src-tauri/templates/
├── en/
│   ├── daily_standup.json
│   └── ...
└── zh-CN/
    ├── daily_standup.json
    └── ...
```

选择顺序建议：

```text
用户明确选择的模板 Locale
        ↓
会议 summaryLanguage
        ↓
uiLocale
        ↓
en
```

模板 ID 必须跨语言保持不变；只翻译名称、描述、章节标题和 Prompt，不翻译结构字段名。模板变更要单独维护版本号，避免应用更新覆盖用户自定义模板。

本次扫描出的 104 条模板字符串已在目录中标记为 `separate_content_localization`，没有直接进入 UI 的 `en.json`。

## 13. 设置页面设计

在 Settings 中增加独立的 “Display Language / 显示语言”：

```text
Display Language
  • System default
  • English
  • 简体中文
```

显示语言应使用自身语言名称，保证用户即使误切语言也能找到：

- English
- 简体中文
- 繁體中文
- 日本語

切换应即时生效，并同步：

- React 文本；
- `<html lang>` 与 `dir`；
- Tauri 托盘；
- 系统通知语言；
- 日期、数字和语言名称格式。

不应改变：

- 当前转录语言；
- 摘要语言；
- 已有会议内容；
- 用户 Prompt；
- 模板显式语言选择。

## 14. 中文翻译风格建议

建议统一术语：

| 英文 | 推荐简体中文 | 说明 |
|---|---|---|
| Meeting | 会议 | 不翻译为“会面” |
| Recording | 录音 / 录制中 | 根据名词或状态选择 |
| Transcript | 转录文本 | 页面空间紧张时可用“转录” |
| Transcription | 转录 | 避免“听写” |
| Summary | 摘要 | AI Summary 为“AI 摘要” |
| Action Items | 行动项 | 团队协作语境 |
| Speaker | 发言人 | 不使用“扬声器” |
| System Audio | 系统音频 | 与麦克风区分 |
| Model | 模型 | Provider 为“服务提供方” |
| Built-in AI | 内置 AI | 保留 AI 大写 |
| Retranscribe | 重新转录 | Enhance 可根据功能译为“增强转录” |
| Confidence | 置信度 | 面向大众也可在说明中解释为识别可信程度 |
| Onboarding | 初始设置 | 用户界面不建议显示“引导流程” |
| Template | 模板 | 摘要模板 |
| Release Notes | 更新说明 | 比“发行说明”更自然 |

语气规范：

- 按钮使用短动词：“保存”“取消”“删除”“重新转录”；
- 进行中状态使用“正在…”：“正在下载…”“正在生成摘要…”；
- 错误先说明结果，再提供动作：“模型下载失败，请重试”；
- 不机械翻译冠词、所有格和英文标题式大小写；
- 中文使用全角中文标点，变量前后避免多余空格；
- “您”与“你”二选一，建议全产品使用“你”，语气更轻量；
- 法律和录音同意提示应单独审校，不能只依赖机器翻译。

## 15. 开发计划总览与阶段验收审计标准

### 15.1 开发计划总览

多语言改造采用“英文基线先行、基础设施先行、前端分批迁移、原生层独立治理、模板内容独立版本、最终统一发布验收”的路线。

| 阶段 | 核心目标 | 主要交付物 | 进入条件 | 阶段放行门禁 |
|---|---|---|---|---|
| 阶段 0 | 冻结和清洗英文基线 | 语义化英文语言包、文本映射表、术语表、原生候选可见性矩阵 | 当前扫描目录已经生成 | 所有文本均有明确处置；无未归类项；英文功能无回归 |
| 阶段 1 | 建立 i18n 运行时基础设施 | i18next 初始化、Provider、Locale Resolver、设置项、格式化工具、回退机制 | 阶段 0 通过 | 英文资源驱动运行；语言选择可持久化；三类语言状态相互独立 |
| 阶段 2 | 完成 React 前端中文化 | `en`/`zh-CN` 正式资源、全部核心页面迁移、前端自动化测试 | 阶段 1 通过 | 简体中文模式下无未批准英文；核心流程端到端通过 |
| 阶段 3 | 完成 Tauri 原生层中文化 | 原生语言包、托盘重建、通知本地化、结构化错误码 | 阶段 1 通过；可与阶段 2 后半段并行 | 托盘、通知、用户错误与 UI Locale 一致；无自由英文错误直出 |
| 阶段 4 | 完成模板和 AI 内容本地化 | 模板 Schema、稳定 ID、英文/中文模板、内容 Locale 选择策略 | 阶段 1 通过；摘要语言模型已明确 | UI、转录、摘要和模板语言互不串扰；所有内置模板双语可用 |
| 阶段 5 | 完成发布前质量审计 | QA 报告、截图基线、平台矩阵、发布包、回滚方案 | 阶段 0–4 全部通过 | 无 P0/P1 缺陷；自动门禁全部通过；人工审校签字完成 |

阶段依赖关系：

```text
阶段 0：英文基线
        │
        ▼
阶段 1：i18n 基础设施
        │
        ├────────► 阶段 2：React 前端迁移 ────────┐
        ├────────► 阶段 3：Tauri 原生层 ─────────┤
        └────────► 阶段 4：模板和 AI 内容 ───────┤
                                                   ▼
                                         阶段 5：统一 QA 与发布
```

阶段 2、3、4 可以在阶段 1 通过后并行，但阶段 5 不得在任何一个阶段存在阻断问题时开始发布放行。

### 15.2 通用验收审计规则

#### 验收结论

每个阶段只能使用以下四种结论：

| 结论 | 定义 | 是否允许进入下一阶段 |
|---|---|---|
| `PASS` | 所有必选检查通过，没有未关闭阻断缺陷 | 允许 |
| `CONDITIONAL PASS` | 仅剩已书面接受的非阻断问题，并有负责人和截止日期 | 经项目负责人批准后允许 |
| `FAIL` | 存在任意阻断缺陷、关键证据缺失或量化指标未达标 | 不允许 |
| `N/A` | 检查项确实不适用于当前平台或功能，并写明理由 | 不影响，但必须审计签字 |

#### 缺陷等级

| 等级 | 定义 | 示例 | 阶段门禁 |
|---|---|---|---|
| `P0` | 数据丢失、安全、隐私、无法启动或核心功能完全不可用 | 切换语言导致录音丢失；安装包无法启动 | 必须为 0 |
| `P1` | 核心流程错误、严重语言串扰、大面积缺失翻译 | 中文 UI 改变转录语言；录音托盘无法停止 | 必须为 0 |
| `P2` | 局部可见错误、布局明显异常、非核心流程失败 | 某个弹窗仍显示英文；中文文本被截断 | 原则上为 0；延期必须书面批准 |
| `P3` | 轻微润色、非阻断视觉差异或低影响一致性问题 | 个别术语可进一步优化 | 可带入后续版本，但必须登记 |

#### 审计证据要求

每个阶段必须保存以下类型的证据，不能只口头确认：

- 自动化命令、运行时间、退出码和完整日志；
- 键集合、占位符、硬编码扫描和未使用键报告；
- 关键流程的英文/中文截图或录屏；
- 测试环境、操作系统、应用版本、Git 提交和构建类型；
- 缺陷清单，包括等级、负责人、状态和处置结论；
- 人工语言审校记录；
- 阶段验收报告和最终结论。

建议证据目录：

```text
docs/i18n/audit/
├── phase-0-baseline/
├── phase-1-infrastructure/
├── phase-2-frontend/
├── phase-3-native/
├── phase-4-templates/
└── phase-5-release/
```

每份阶段报告至少包含：

```text
阶段名称：
验收版本 / Git Commit：
验收日期：
验收环境：
检查项总数：
通过 / 失败 / N/A 数量：
P0 / P1 / P2 / P3 数量：
遗留风险：
证据链接：
验收结论：PASS / CONDITIONAL PASS / FAIL
审核人：
批准人：
```

#### 通用阻断条件

任意阶段出现以下情况必须判定为 `FAIL`：

- JSON 无法解析、语言包无法加载或应用无法启动；
- 英文基线缺键且没有回退；
- 英文和中文占位符不一致；
- 切换 UI Locale 会改变转录语言或摘要语言；
- 语言切换导致录音、转录、摘要、模型下载或编辑状态丢失；
- 中文模式出现翻译键原文，例如 `recording.actions.start`；
- 用户可见错误直接展示堆栈、内部路径、API Key 或隐私数据；
- 存在未批准的 P0/P1 缺陷；
- 缺少要求的测试日志、截图或审核记录。

### 15.3 阶段 0：冻结英文基线

> 执行状态：**PASS**。冻结基线基于 Git Commit `0281737d87d26352fb0adc78c8c0975f691b23d1`；39/39 项自动静态审计、11/11 项严格运行时审计和前端生产构建全部通过，P0/P1/P2 为 0。英文烟测、React 受控编辑、Enter 保存、Escape 取消、鼠标/键盘捕获与冒泡、嵌套编辑父级导航保护、Rust→Tauri→JS 错误传播和用户数据恢复均有落盘证据。三个非阻断 P3 已登记负责人和关闭门禁。正式报告：`docs/i18n/audit/phase-0-runtime/phase0-runtime-audit-report.md`。

#### 目标

把自动扫描结果转化为可维护、可追踪的正式英文翻译基线，并明确每条文本的最终处置方式。

#### 主要任务

- 将 `en.catalog.json` 纳入开发基线；
- 人工处理 62 条前端复核项；
- 把 20 条复杂源码表达式改成简单插值变量；
- 将自动文件型键归并为稳定语义键；
- 合并通用动作和状态，拆分语境不同的同词项；
- 建立不可翻译项允许列表；
- 建立英文术语表和中文术语决策表；
- 确认 447 条原生候选是否会到达用户界面；
- 将 45 条旧代码项维持为排除状态，避免混入运行时资源；
- 为 104 条模板内容建立独立内容本地化清单。

#### 必须交付

- 正式英文资源包及 Namespace 划分；
- “源码出现位置 → 正式翻译键”映射表；
- 文本状态处置表；
- 占位符清单；
- 不翻译项允许列表；
- 英文/中文术语表；
- Rust/Tauri 用户可见性矩阵；
- 阶段 0 审计报告。

#### 验收审计标准

| 审计项 | 通过标准 | 审计方法 | 必备证据 |
|---|---|---|---|
| 原始范围覆盖 | 1549 个原始出现位置全部映射、合并或明确排除，未处置数为 0 | 对比 `source-text-candidates.json` 与映射表 | 覆盖率报告 |
| 去重目录处置 | 1354 条目录记录均有最终状态，无空状态、无未知状态 | JSON 校验脚本 | 状态统计报告 |
| 前端翻译覆盖 | 696 条确认项全部映射到正式语义键；允许合并，但来源映射不得丢失 | 反向检查每个 source occurrence | 键映射表 |
| 前端复核项 | 62 条全部标记为翻译、排除或开发日志，未决数为 0 | 人工逐条复核 | 复核签字表 |
| 原生可见性 | 447 条原生候选全部分类为用户可见、仅日志、内部错误或旧代码 | 调用链审计 | 原生可见性矩阵 |
| 复杂占位符 | 20 条复杂表达式全部改为简单变量；语言包内不含函数调用、三元表达式或对象属性链 | 占位符静态扫描 | 占位符报告 |
| 键命名 | 100% 使用语义键；不以完整英文原文作键；不依赖具体组件文件名 | 键规则 Lint + 人工抽查 | 键命名报告 |
| 重复文本 | 重复动作完成归并；同词不同语境保留独立键并注明原因 | 重复值报告 | 合并决策表 |
| 品牌和协议 | 产品名、模型名、URL、命令名、存储键和协议字段进入允许列表 | 允许列表扫描 | `do-not-translate` 清单 |
| 英文回归 | 使用英文资源运行时，与改造前的可见文本和功能语义一致 | 英文截图对比和冒烟测试 | 前后对比截图、测试日志 |
| 构建状态 | 前端生产构建通过；现有测试不新增失败 | 执行构建与测试 | 完整日志、退出码 0 |

#### 阶段 0 阻断条件

- 任意确认翻译项找不到正式键或源码映射；
- 任意复杂表达式仍直接存在于正式语言包；
- 原生候选存在“是否显示给用户”未决项；
- 英文资源驱动后出现功能或文本回归；
- 术语表未确定 Meeting、Recording、Transcript、Summary、Speaker、Model 等核心词。

#### 阶段 0 放行标准

- 全部审计项为 `PASS` 或经批准的 `N/A`；
- 覆盖率为 100%；
- 未处置文本为 0；
- P0/P1/P2 为 0；
- 英文基线、映射表和术语表完成审核签字。

### 15.4 阶段 1：搭建 i18n 基础设施

> 执行状态：**PASS（已放行）**。i18n 专项测试 12/12、Production 真实切换 10/10、Production 冷启动 4/4、Tauri dev 4/4、最终 release 冷启动回归 4/4，TypeScript、Next 12/12 静态页面和 Windows x64 Tauri release 均通过；P0/P1/P2 为 0。官方 LLVM x64 便携工具链已关闭 `libclang.dll` 环境阻断，真实用户数据已完成隔离审计和原样恢复。完整报告：`docs/i18n/audit/phase-1-runtime/phase1-audit-report.md`。阶段 2 门禁已开放。

#### 实际验收结论

| 验收域 | 实际结果 | 审计证据 |
|---|---|---|
| Locale 与 Provider | PASS | 12/12 i18n 专项测试；运行时键盘切换与重挂载持久化 |
| 状态隔离 | PASS | 转录、摘要、Provider 模型、真实模型下载、编辑输入和录音状态快照 |
| 首屏与持久化 | PASS | 两轮 Production 冷启动均 4/4，无错误语言可见帧 |
| 离线与回退 | PASS | 禁网运行、静态资源加载、缺失中文键英文回退 |
| Tauri dev | PASS | Turbopack + `devCsp`；页面、bootstrap、IPC、CSP 4/4 |
| Tauri release | PASS | Meetily 0.4.0 PE x64；SHA-256 `2B53AC3781F5586AA45764F2640C6C6E294D87D6E023F00CE77D09E798B3F050` |
| 用户数据安全 | PASS | 三个真实数据目录回迁完成；审计数据独立保留，未删除 |

阶段 1 的放行只代表国际化架构和显示语言设置切片已稳定，不代表 React 全界面、Tauri 托盘/通知或模板/AI 内容已经完成中文化；这些内容仍按阶段 2、3、4 的验收标准分别执行。

#### 目标

在不改变现有英文功能的前提下，让应用完全通过国际化运行时加载英文资源，并具备可靠的 Locale 解析、切换、持久化和回退能力。

#### 主要任务

- 安装并锁定 `i18next` 与 `react-i18next`；
- 创建 `I18nProvider`、Locale Resolver、类型声明和格式化工具；
- 注册 `en` 和 `zh-CN`，英文作为最终回退；
- 增加独立的 `meetily.uiLocale`；
- 实现 `system`、`en`、`zh-CN` 选择；
- 动态更新 `<html lang>` 和 `dir`；
- 在 Settings 增加显示语言设置；
- 增加开发环境缺失键告警；
- 确保 Locale 资源随安装包离线提供，不依赖网络；
- 保持 `primaryLanguage` 和摘要语言的现有业务行为不变。

#### 必须交付

- i18n 初始化模块和 React Provider；
- Locale 类型、支持列表、解析器和持久化逻辑；
- 日期、时间、数字、百分比、相对时间和语言名称格式化工具；
- Settings 显示语言组件；
- Locale 解析和切换测试；
- 阶段 1 审计报告。

#### 验收审计标准

| 审计项 | 通过标准 | 审计方法 | 必备证据 |
|---|---|---|---|
| 依赖可复现 | `package.json` 和锁文件同步；全新安装后构建成功 | 删除依赖缓存后的冻结锁安装与构建 | 安装及构建日志 |
| Locale 解析 | `system`、`en`、`zh-CN`、`zh-CN` 系统区域、`en-US` 和未知 Locale 均符合回退规则 | Locale Resolver 单元测试 | 测试报告 |
| 持久化 | 选择语言后重启应用仍保持；`system` 模式继续跟随系统 | 切换、退出、重启测试 | 操作录屏或步骤记录 |
| 运行时切换 | 切换语言无需重启，不丢失当前页面和业务状态 | 录音前、编辑中、模型下载中分别切换 | 状态对比记录 |
| HTML 元数据 | `html.lang` 与当前实际 Locale 一致；`dir` 为正确方向 | DOM 自动断言 | 测试日志 |
| 英文回退 | 删除一个测试用中文键时只回退英文，不显示键名、不崩溃 | 缺失键注入测试 | 截图和日志 |
| 三语言状态隔离 | 改变 `uiLocale` 后，转录语言和摘要语言前后完全一致 | 状态快照对比 | 自动测试结果 |
| 离线可用 | 禁网环境下 Locale 资源正常加载 | 禁用网络运行应用 | 录屏/日志 |
| 格式化工具 | 日期、数字、百分比、相对时间和语言名称在 `en`/`zh-CN` 下输出正确 | 单元测试固定样例 | 测试结果 |
| 首屏稳定 | 启动时不先闪英文再切中文；不出现未翻译键 | 中文持久化后冷启动录屏 | 首屏录屏 |
| 构建兼容 | Next.js 静态导出、Tauri dev 和生产构建均通过 | 执行三类构建 | 完整日志 |

#### 阶段 1 阻断条件

- Locale 初始化依赖网络；
- `system` 或未知 Locale 导致空白页；
- 切换 UI Locale 修改 `primaryLanguage`、转录 Provider、摘要语言或模型设置；
- 冷启动出现明显语言闪烁；
- 缺失键显示内部键名或导致 React 异常；
- Tauri 静态构建无法包含语言资源。

#### 阶段 1 放行标准

- Locale Resolver 测试覆盖率 100%；
- 所有支持 Locale 的加载与回退测试通过；
- 三类语言隔离测试全部通过；
- Next、Tauri 开发和生产构建退出码均为 0；
- P0/P1/P2 为 0。

### 15.5 阶段 2：迁移 React 核心用户路径

> 执行状态：**CONDITIONAL PASS（2A–2F 实施批次完成，Windows x64 对应前端 UI/状态范围 PASS）**。六个 React 批次均已完成严格验收；2F 最终资源键为 `common` 75/75、`analytics` 63/63、`updates` 34/34，资源与已迁移源码门禁 0/0，i18n 自动测试 46/46，会议模板并行回归 39/39，真实 Tauri/WebView2 运行时 17/17，Runtime exception / unexpected Console error / Log error / CSP 均为 0。分析默认关闭、显式 opt-in、失败时 UI 与持久 store 双回滚、隐私透明度、更新有/无/失败、日期格式、About、Beta 与热切换状态均已验收；Next.js 13/13 页面与隔离 Tauri Release 构建通过。2F 最终审计二进制 SHA-256 为 `125C85D19BDEE9DE0A743B2100C8C2961A631DBD953503ED3E25DC71D651711E`，构建后 21 个产品源/资源漂移为 0。真实设备/模型/媒体、真实更新下载安装与重启、当前未挂载组件、灾难恢复和非 Windows 矩阵仍是条件保留，因此阶段 2 不得解释为最终发布 PASS。2F 正式报告：`docs/i18n/audit/phase-2-react/2F/phase2-2F-audit-report.md`；下一执行阶段为 **15.6 阶段 3：Tauri 原生层国际化**。

#### 目标

将所有用户可见前端英文迁入正式资源包，完成简体中文翻译，并保证主要业务流程在两种语言下行为一致。

#### 主要任务

- 按 2A–2F 六个批次迁移全部核心用户路径；
- 将 JSX、Toast、Dialog、Tooltip 和辅助功能文本替换为正式翻译键；
- 将 Hook、Context 和服务层产生的用户状态与错误接入 i18n；
- 完成英文和简体中文资源，执行术语、标点和语气审校；
- 将日期、时间、数字、百分比和语言名称切换到 Locale 格式化工具；
- 为每个批次补充自动测试、截图和硬编码扫描证据；
- 保证运行时切换语言不会重置或中断任何业务状态。

#### 迁移批次

| 批次 | 范围 | 核心场景 |
|---|---|---|
| 2A | 首次启动、权限、模型下载 | Welcome、权限申请、模型选择、下载、失败重试 |
| 2B | 侧栏、首页、录音、实时转录 | 导航、开始/暂停/继续/停止、Listening、录音状态 |
| 2C | 会议列表、详情、摘要（Windows x64 对应范围已 CONDITIONAL PASS） | 标题编辑与保存、转录复制、摘要生成/停止/重试、会议模板、摘要语言与 UI Locale 状态隔离 |
| 2D | 设置、模型、音频设备（Windows x64 对应范围已 CONDITIONAL PASS） | Provider、API 配置、Ollama、Whisper、Parakeet、设备与路径 |
| 2E | 导入、重新转录、恢复（Windows x64 前端 UI/状态范围已 CONDITIONAL PASS） | 文件选择与拖放、格式、进度、取消失败、恢复中断会议、数据库迁移组件 |
| 2F | 更新、分析授权、About、Beta（Windows x64 前端 UI/状态范围已 CONDITIONAL PASS） | 版本、更新说明、隐私说明、分析数据和辅助功能 |

#### 2D 实际验收结论

| 验收域 | 实际结果 | 审计证据 |
|---|---|---|
| 英中资源与英文基线 | PASS | `settings` 173/173、`models` 218/218；阶段 0 冻结英文键和值逐项保持 |
| 设置与模型源码 | PASS | 16 个 2D 产品源文件；资源失败 0、源码失败 0、未批准英文 0 |
| 状态与错误隔离 | PASS | 设备失败 UI/后端回滚；模型/设备原始错误不冒泡；Secret 不进入 DOM/Toast/证据 |
| UI Locale 热切换 | PASS | Provider、端点、模型、设备与路径状态在 `zh-CN ⇄ en` 无刷新切换中保持 |
| 可访问性与布局 | PASS | 四个设置开关补齐本地化名称；1440×900 无横向溢出和裁切 |
| 自动回归 | PASS | i18n 30/30；2D 专属 6/6；会议模板并行回归 39/39 |
| 构建与运行时 | PASS | Next 13/13；Windows x64 Tauri Release；CDP 12/12；构建后漂移 0 |
| 条件保留 | CONDITIONAL | 物理设备、真实模型传输/推理、macOS/非 Windows、Bun 专属测试与最终发布仍待对应矩阵 |

2D 没有接管 2F 的 Usage Analytics、分析透明度弹窗、About、Beta 与更新文案；这些可见英文保持为 2F 的明确待办，不能据此把阶段 2 整体标记为 PASS。

#### 2E 实际验收结论

| 验收域 | 实际结果 | 审计证据 |
|---|---|---|
| 英中资源与英文基线 | PASS | `import` 106/106、`transcription` 126/126；阶段 0 冻结英文键和值逐项保持 |
| 导入、重新转录与恢复源码 | PASS | 11 个 2E 产品源文件；资源失败 0、源码失败 0、未批准英文 0 |
| 文件入口与失败安全 | PASS | 拖入/离开覆盖层、文件选择、非法文件受控错误、后端哨兵不进入 UI |
| 进度与取消状态机 | PASS | 导入/重新转录稳定阶段码；取消失败保持活动且可再次成功取消 |
| 恢复数据安全 | PASS | 后端保存成功后才标记 IndexedDB；失败保留数据；删除需确认 |
| UI Locale 热切换 | PASS | 文件、用户标题、模型、进度、会议标题和恢复记录在 `zh-CN ⇄ en` 中保持 |
| 可访问性与布局 | PASS | 修复折叠 Logo 的可访问名称；1440×900 无横向溢出、裁切或无名按钮 |
| 自动回归 | PASS | i18n 38/38；2E 专属 8/8；会议模板并行回归 39/39 |
| 构建与运行时 | PASS | Next 13/13；Windows x64 Tauri Release；CDP 16/16；构建后漂移 0 |
| 条件保留 | CONDITIONAL | 真实媒体推理、未挂载的旧数据库迁移、灾难恢复、非 Windows 与最终发布仍待对应矩阵 |

`LegacyDatabaseImport` 当前没有产品挂载点，所以数据库迁移组件通过了资源、错误隔离、构建和静态契约审计，但没有被错误宣称为用户路由端到端通过。

#### 2F 实际验收结论

| 验收域 | 实际结果 | 审计证据 |
|---|---|---|
| 英中资源与英文基线 | PASS | `common` 75/75、`analytics` 63/63、`updates` 34/34；冻结英文逐项保持 |
| 更新、分析、About、Beta 源码 | PASS | 15 个清单源文件；资源失败 0、源码失败 0；仅 3 条精确品牌/技术示例允许项 |
| 分析授权与失败安全 | PASS | 默认关闭；显式 opt-in；初始化失败时 UI、store 和部分初始化同时回滚；原始哨兵不进入 UI |
| 更新发现与格式化 | PASS | 无更新、检查失败、有更新、版本/说明、中文/英文日期格式和受控错误全部通过 |
| UI Locale 热切换 | PASS | 更新版本、授权、用户 ID、透明度弹窗和 Beta 状态在 `zh-CN ⇄ en` 中保持 |
| 可访问性与布局 | PASS | About/Logo/开关/弹窗/合规关闭控件有名称；1440×900 无横向溢出、裁切或无名按钮 |
| 自动回归 | PASS | i18n 46/46；2F 专属 8/8；会议模板并行回归 39/39 |
| 构建与运行时 | PASS | Next 13/13；Windows x64 Tauri Release；CDP 17/17；构建后漂移 0/21 |
| 条件保留 | CONDITIONAL | 真实更新下载/安装/重启、未挂载合规组件、非 Windows 与最终发布仍待对应矩阵 |

阶段 2 的 2A–2F 实施批次现已全部完成并记录为 **CONDITIONAL PASS**；由于上述条件保留，它不是最终发布 PASS。下一执行阶段为 **15.6 阶段 3：Tauri 原生层国际化**。

#### 每个文件的强制迁移清单

- JSX 文字节点；
- Toast 标题和说明；
- Dialog、Alert、Popover、Tooltip；
- `title`、`placeholder`、`alt`、`aria-label`；
- Hook 和 Context 返回的用户状态与错误；
- 按钮进行中状态和条件表达式；
- 日期、时间、数字、百分比和语言名称；
- 空状态、错误状态、加载状态和禁用原因；
- 英文和中文键；
- 单元测试、组件测试或端到端测试；
- 硬编码扫描基线更新。

#### 必须交付

- 正式 `en` 与 `zh-CN` 前端资源；
- 六个迁移批次的覆盖清单；
- 关键流程端到端测试；
- 英文/中文截图基线；
- 中文人工审校记录；
- 未翻译允许列表；
- 阶段 2 审计报告。

#### 通用验收审计标准

| 审计项 | 通过标准 | 审计方法 | 必备证据 |
|---|---|---|---|
| 键集合 | `en` 与 `zh-CN` 翻译键集合 100% 一致 | 递归键比较 | CI 报告 |
| 占位符 | 每个键的占位符名称和数量 100% 一致 | 占位符对比脚本 | CI 报告 |
| 空值 | 中文包无空字符串、`null`、`TODO`、机器占位符或直接复制的待翻英文 | 内容规则扫描 | 扫描报告 |
| 硬编码 | 已迁移范围内未批准的用户可见英文为 0 | AST 增量扫描 + 中文运行遍历 | 硬编码报告 |
| 键名泄露 | 页面、Toast、Tooltip、控制台用户错误中不出现翻译键 | 自动搜索 DOM 和截图复核 | 测试日志 |
| 可访问文本 | `aria-label`、`title`、`alt` 与可见语言一致 | DOM 与辅助功能扫描 | a11y 报告 |
| 状态一致 | 语言切换前后路由、会议、录音、模型下载和编辑状态不变 | 状态快照测试 | 测试记录 |
| 英文回归 | 英文模式功能与迁移前一致 | 核心流程回归 | 英文测试报告 |
| 中文语言质量 | 核心术语符合术语表；中文标点、语气和变量位置正确 | 双人语言审校 | 审校签字表 |
| 布局 | 目标窗口尺寸下无截断、遮挡、重叠或不可点击控件 | 多尺寸截图比较 | 截图集 |

#### 各批次验收场景

| 批次 | 必测通过场景 | 失败即阻断的情况 |
|---|---|---|
| 2A | 新用户首次启动、权限允许/拒绝、下载成功/失败/取消/重试 | 无法完成初始设置；权限说明错误；下载失败无可操作提示 |
| 2B | 开始、暂停、继续、停止录音；麦克风和系统音频状态；实时转录空/忙/错误状态 | 语言切换中断录音；停止按钮不可用；状态文本与真实状态不一致 |
| 2C | 会议标题编辑与保存、转录复制、模板选择/失败恢复、摘要生成/停止/重试/保存、摘要语言切换、UI Locale 热切换 | 会议内容丢失；标题无法编辑或保存；摘要目标语言被 UI Locale 覆盖；后端错误冒泡；复制、模板或保存失败 |
| 2D | Provider 切换、API Key 输入、模型搜索/下载/删除、设备测试、存储路径显示 | Secret 泄露；模型配置被语言切换重置；设备选择变化 |
| 2E | 拖放和选择文件、非法格式、导入进度、重新转录、恢复/删除中断会议 | 文件误删；导入状态丢失；恢复流程不可用 |
| 2F | 更新有/无可用版本、分析开关、隐私说明、About、Beta 开关 | 隐私含义翻译错误；分析默认值变化；更新操作失效 |

#### 阶段 2 阻断条件

- 中文模式核心流程出现未批准英文；
- 任意英文/中文键或占位符不一致；
- Locale 切换造成录音、导入、下载、编辑或摘要生成状态丢失；
- 隐私、录音同意、分析授权或删除确认的中文语义不准确；
- UI 中显示内部错误、代码表达式或翻译键；
- 六个批次中任一批次的必测场景未通过。

#### 阶段 2 放行标准

- 六个批次覆盖率均为 100%；
- 所有已迁移源文件的未批准可见英文为 0；
- 键集合、占位符和空值检查全部通过；
- 核心流程英文/中文端到端测试全部通过；
- 中文人工审校完成；
- P0/P1/P2 为 0。

### 15.6 阶段 3：Tauri 原生层国际化

> 执行状态：**CONDITIONAL PASS（Windows x64 自动化实施与真实运行时通过，阶段总门禁尚未关闭）**。原生 `en`/`zh-CN` 资源、`set_ui_locale`/`get_ui_locale`、重启持久化、托盘热重建、8 类托盘状态规格、7 类通知本地化、结构化错误契约和 492 项候选处置矩阵已经落地；447 条在线候选分类完成、45 条旧代码均排除、未决为 0。阶段 3 静态审计 17/17，前端 i18n 49/49，Rust 全库 270 通过/0 失败/2 ignored，`tsc --noEmit` 0 error，Next 13/13，Windows WebView2 真实运行时 9/9，构建后完整性 4/4，Runtime/Log/Console 错误均为 0。最终隔离 EXE SHA-256 为 `DDBD8BC84BDBDE4D45E07C15335DA221CC2A3AAC68E13B2D0C61A1890E6BE409`；正式 `target/release/meetily.exe` 保持冻结哈希 `1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823`，未被覆盖。Windows 托盘逐状态截图、通知截图以及 macOS/Linux 测试或发布范围签字仍为 OPEN，因此不得写成最终 `PASS`，也不自动进入阶段 4。正式报告：`docs/i18n/audit/phase-3-native/phase3-audit-report.md`。

#### 目标

让托盘菜单、系统通知和用户可见 Rust 错误与 UI Locale 保持一致，同时保留稳定菜单 ID、技术日志和诊断能力。

#### 主要任务

- 建立 `en` / `zh-CN` 原生语言包；
- 实现 `set_ui_locale` 与 `get_ui_locale`；
- Locale 改变时安全重建托盘菜单；
- 本地化所有录音状态菜单；
- 本地化后台模型下载状态和系统通知；
- 将用户错误改成 `code + params + debugMessage`；
- 确保日志不因翻译而失去可检索性；
- 清理或隔离 `_old` / `old_complex` 源文件。

#### 必须交付

- Rust 原生 Locale 加载模块；
- 原生英文/中文资源；
- 结构化错误 Schema 和错误码注册表；
- 原生候选最终处置矩阵；
- 托盘、通知、错误映射测试；
- 阶段 3 审计报告。

#### 验收审计标准

| 审计项 | 通过标准 | 审计方法 | 必备证据 |
|---|---|---|---|
| 原生候选处置 | 447 条候选全部完成用户可见性分类和代码处置，未决数为 0 | 矩阵与源码反查 | 最终矩阵 |
| 旧代码隔离 | 45 条旧代码项不会进入运行时资源或扫描门禁 | 编译模块和扫描规则检查 | 排除报告 |
| 托盘完整性 | 空闲、下载、启动、录音、暂停、继续、停止状态在英文/中文下均正确 | 状态机逐状态测试 | 每状态截图 |
| 菜单行为 | 翻译前后菜单 ID、事件和动作一致；语言切换不改变行为 | 自动事件测试 | 测试日志 |
| 托盘热切换 | 应用运行时切换 Locale 后托盘立即更新，无重复托盘、崩溃或丢失动作 | 连续切换压力测试 | 录屏和进程日志 |
| 通知 | 录音开始/停止、下载和错误通知均使用当前 Locale | 触发矩阵测试 | 通知截图 |
| 错误码覆盖 | 所有用户可见 Rust 错误都有稳定错误码和英文/中文映射 | 错误注册表对比 | 覆盖报告 |
| 未知错误回退 | 未注册错误显示安全通用消息，技术细节只进入日志 | 注入未知错误 | 截图和日志 |
| 隐私安全 | 用户消息不含 API Key、内部绝对路径、堆栈和敏感会议内容 | 敏感信息扫描 | 安全审计报告 |
| 日志可诊断 | 翻译不改变机器可检索错误码；英文 debugMessage 可用于技术诊断 | 日志抽查 | 示例日志 |
| 重启持久化 | 原生 Locale 与 UI Locale 在重启后保持一致 | 退出/重启测试 | 操作记录 |

#### 平台验收矩阵

| 平台 | 托盘 | 通知 | Locale 热切换 | 启动持久化 | 必须结论 |
|---|---|---|---|---|---|
| Windows x64 | 必测 | 必测 | 必测 | 必测 | `PASS` |
| macOS Intel/Apple Silicon | 支持发布时必测 | 支持发布时必测 | 必测 | 必测 | `PASS` 或有批准的发布范围说明 |
| Linux AppImage/deb | 支持发布时必测 | 支持发布时必测 | 必测 | 必测 | `PASS` 或有批准的发布范围说明 |

#### 阶段 3 阻断条件

- 中文 UI 下托盘或通知仍出现未批准英文；
- 切换语言生成重复托盘或丢失停止录音能力；
- 任意用户错误直接展示 Rust Debug、堆栈、绝对路径或 Secret；
- 用户可见错误仍依赖自由文本匹配；
- 未知错误没有安全回退；
- 原生 Locale 与 UI Locale 在重启后不一致。

#### 阶段 3 放行标准

- 原生候选处置率 100%；
- 所有托盘状态的英文/中文矩阵通过；
- 用户错误码覆盖率 100%；
- 敏感信息泄露数为 0；
- 当前正式发布平台全部通过或有书面范围豁免；
- P0/P1/P2 为 0。

#### 本轮验收记录（2026-08-23）

- 自动化实现与 Windows 真实运行时：`PASS`；
- 原生候选处置：447/447；旧代码隔离：45/45；未决：0；
- 阶段 3 新增 Rust 测试：12/12；Rust 全库：270 通过、0 失败、2 ignored；
- 前端 i18n：49/49；静态门禁：17/17；`tsc --noEmit`：0 error；Next：13/13；运行时：9/9；构建后完整性：4/4；
- 条件保留：Windows Shell 托盘/通知截图；macOS/Linux 测试或书面发布范围；
- 阶段结论：`CONDITIONAL PASS`，阶段 4 尚未获自动放行。

### 15.7 阶段 4：内置模板和 AI 内容本地化

> 执行状态：**CONDITIONAL PASS（Windows x64 实施、全库回归和真实运行时通过，双人语言审校门禁尚未关闭）**。6 个稳定内置模板已建立 `en` / `zh-CN` 共 12 个 V2 资源，104/104 条内容候选已处置、未决为 0；内容 Locale 由 `summaryLanguage` 决定并安全回退英文，UI Locale 只影响展示；来源优先级为“用户自定义 > 新版多语言内置 > 旧版 bundled”，历史快照保持不可变。静态审计 18/18、前端 i18n 54/54、模板／生成库 41/41、Rust 全库 278 通过/0 失败/2 ignored、TypeScript 0 error、Next 13/13、Windows WebView2 12/12、构建后完整性 5/5。阶段 4 独立 EXE SHA-256 为 `5B2EDE51C7CE12D40F3945486869A445A2566D85F3AA033490911E813ED27F8B`；正式 `target/release/meetily.exe` 保持冻结哈希 `1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823`。第二位人工语言审校、精神科术语复核与真实 LLM 输出样例仍为 OPEN，因此不得写成最终 `PASS`。正式报告：`docs/i18n/audit/phase-4-content/phase4-audit-report.md`。

#### 目标

为内置模板和 AI Prompt 建立独立内容 Locale、稳定 ID 和版本控制，并保证它们不会与 UI、转录和摘要语言发生错误耦合。

#### 主要任务

- 定义模板 JSON Schema；
- 给现有模板增加稳定 `id`、`locale` 和 `version`；
- 建立 `en` / `zh-CN` 目录；
- 翻译模板名称、描述、章节标题和 Prompt；
- 确定模板 Locale 选择与回退规则；
- 保护用户自定义模板，避免升级覆盖；
- 确保 Prompt 明确服从 `summaryLanguage`；
- 为旧模板和已有会议建立兼容迁移逻辑。

#### 必须交付

- 模板 JSON Schema；
- 全部内置模板的英文和中文版本；
- 模板 ID/Locale/Version 清单；
- 内容 Locale 解析器；
- 旧模板兼容和用户模板保护方案；
- Prompt 语言行为测试；
- 阶段 4 审计报告。

#### 验收审计标准

| 审计项 | 通过标准 | 审计方法 | 必备证据 |
|---|---|---|---|
| Schema | 所有模板通过 JSON Schema；缺失 `id`、`locale`、`version` 时构建失败 | Schema CI | 校验日志 |
| ID 稳定性 | 同一模板的英文/中文 `id` 完全一致且全局唯一 | 唯一性脚本 | ID 清单 |
| 结构一致 | 同一模板各 Locale 的章节结构和必需字段一致 | 结构对比 | 差异报告 |
| 内容覆盖 | 104 条内容候选全部映射到模板字段、明确排除或标为不翻译，未处置数为 0 | 内容目录反查 | 内容覆盖报告 |
| 中文完整性 | 所有内置模板均有 `zh-CN` 版本，无空字段和待翻标记 | 内容扫描 | 完整性报告 |
| 回退 | 中文模板缺失的模拟场景安全回退到英文，不崩溃、不显示路径 | 缺失资源注入测试 | 测试日志 |
| 语言隔离 | 切换 `uiLocale` 不修改已选择模板 Locale、转录语言或摘要语言 | 状态矩阵测试 | 状态对比 |
| 摘要服从性 | 模板 Prompt 不会覆盖用户明确选择的 `summaryLanguage` | 多组合生成测试 | 输出样例 |
| 用户数据保护 | 应用升级和 Locale 切换不覆盖用户自定义模板 | 升级模拟测试 | 文件哈希和日志 |
| 旧数据兼容 | 旧会议和旧模板可继续打开；无 ID 时有确定迁移规则 | 旧数据样本测试 | 兼容性报告 |
| 语言质量 | 中文模板自然、术语一致，不改变业务目的和摘要结构 | 内容审校 | 审校签字表 |

#### 语言组合测试矩阵

至少验证以下组合：

| UI Locale | 转录语言 | 摘要语言 | 模板 Locale | 预期 |
|---|---|---|---|---|
| `zh-CN` | `zh` | `zh` | `zh-CN` | 中文 UI、中文模板、中文摘要 |
| `zh-CN` | `ja` | `zh` | `zh-CN` | 中文 UI、日语转录、中文摘要 |
| `zh-CN` | `en` | `en` | `en` | 中文 UI、英文模板、英文摘要 |
| `en` | `zh` | `zh` | `zh-CN` | 英文 UI、中文模板、中文摘要 |
| `en` | `auto` | `__auto__` | `en` | 英文 UI，摘要语言由转录主语言决定 |

#### 阶段 4 阻断条件

- UI Locale 会强制改变模板或摘要语言；
- 中文模板改变原模板业务结构或遗漏重要 Prompt 约束；
- 更新覆盖用户自定义模板；
- 旧会议或旧模板无法读取；
- 任意内置模板缺少稳定 ID、Locale 或版本；
- 内容候选存在未处置项。

#### 阶段 4 放行标准

- 模板 Schema 通过率 100%；
- 内置模板中英文覆盖率 100%；
- 内容候选处置率 100%；
- 语言组合矩阵全部通过；
- 用户模板覆盖事件为 0；
- Prompt 和中文内容完成双人审校；
- P0/P1/P2 为 0。

### 15.8 阶段 5：中文 QA、发布与回滚

#### 目标

对阶段 0–4 的结果进行统一端到端验收，生成可发布安装包，并保证出现问题时能够安全回退到英文或上一版本。

#### 主要任务

- 完成英文/中文全流程回归；
- 完成支持平台的字体、托盘和通知测试；
- 完成窗口尺寸、缩放和高 DPI 截图审计；
- 完成隐私、分析授权、录音同意和删除确认的中文法律语义复核；
- 验证离线模式、英文回退、未知错误和资源缺失；
- 验证安装、覆盖升级、卸载和用户数据保留；
- 生成签名或明确标识的发布包；
- 制定运行时回退和版本回滚方案。

#### 必须交付

- 发布候选安装包和文件哈希；
- 平台测试矩阵；
- 英文/中文端到端报告；
- 截图和布局审计报告；
- 可访问性报告；
- 隐私/法律文案审核记录；
- 安装、升级、卸载和数据保留报告；
- 已知问题和风险接受清单；
- 回滚操作手册；
- 最终发布验收报告。

#### 功能验收审计标准

| 领域 | 通过标准 | 必测场景 |
|---|---|---|
| 启动与设置 | 首次和非首次启动均正确加载 Locale | 新装、已有配置、`system`、英文、中文 |
| 权限 | 权限允许、拒绝和稍后处理均有正确中英文说明 | 麦克风、系统音频、通知 |
| 模型 | 模型检查、下载、取消、重试、删除和选择无回归 | Whisper、Parakeet、内置摘要模型 |
| 录音 | 全状态可操作，语言切换不影响录音 | 开始、暂停、继续、停止、后台托盘 |
| 转录 | 实时文本、置信度、空状态和错误正确 | 中文、英文、自动识别、翻译模式 |
| 会议 | 列表、搜索、标题、删除、恢复和打开目录正常 | 新会议、旧会议、中断会议 |
| 摘要 | 生成、停止、重试、保存、复制和语言选择正常 | 多 Provider、多模板、多语言组合 |
| 导入 | 格式校验、进度、失败、重试和重转录正常 | MP3/WAV/MP4、非法文件、取消 |
| 更新 | 有更新、无更新、失败和重试正确 | 在线、离线、无权限 |
| 隐私 | 分析授权默认值不变，说明准确，敏感信息不泄露 | 开启、关闭、查看收集内容 |
| 原生层 | 托盘、通知、退出和窗口恢复正确 | 英文/中文、各录音状态 |

#### 视觉和可访问性验收标准

| 审计项 | 通过标准 |
|---|---|
| 窗口尺寸 | 最小支持尺寸、默认尺寸和最大化状态下无阻断性截断 |
| 缩放 | Windows 100%、125%、150%、200% 缩放下核心操作可见可点击 |
| 字体 | 中文无方框、乱码或异常字重；中英文混排基线一致 |
| 文本溢出 | 按钮、Tab、Select、Dialog、Toast、托盘无不可读截断 |
| 键盘操作 | 语言切换后焦点顺序和快捷键不变 |
| 屏幕阅读器 | 控件名称和状态使用当前 Locale，不重复、不为空 |
| 对比度 | 中文化未引入颜色或状态识别回归 |
| RTL 预备 | `dir` 架构可工作；首期未支持 RTL 时可标 `N/A`，但不得硬编码破坏方向切换 |

#### 性能与稳定性验收标准

| 审计项 | 通过标准 |
|---|---|
| 启动性能 | 相对英文改造前基线，中位冷启动退化不超过 10% |
| 内存 | 空闲和录音场景相对基线无持续增长；语言反复切换后无明显泄漏 |
| 切换稳定性 | 连续切换英文/中文 50 次无崩溃、重复 Provider、重复托盘或状态丢失 |
| 长时录音 | 至少完成一次 60 分钟录音并在期间切换 UI Locale，录音和保存正常 |
| 离线资源 | 禁网后所有语言资源、托盘和错误回退可用 |
| 缺失资源 | 模拟单个中文 Namespace 缺失时回退英文且记录一次明确告警 |

#### 安装、升级和回滚验收标准

| 审计项 | 通过标准 |
|---|---|
| 全新安装 | 安装后可启动，默认 Locale 解析正确 |
| 覆盖升级 | 从现有 0.4.0 升级后会议、设置、模型和录音目录不丢失 |
| 设置迁移 | 没有 `meetily.uiLocale` 的旧用户安全默认到系统或英文 |
| 卸载 | 卸载程序可用；用户数据是否保留与产品策略一致并有说明 |
| 英文应急回退 | 中文资源损坏或加载失败时自动使用英文，不阻止应用启动 |
| 版本回滚 | 回退上一版本后旧会议和配置仍可读取；新增 Locale 键不会破坏旧版 |

#### 阶段 5 阻断条件

出现以下任一情况，阶段 5 必须判定为 `FAIL`，不得生成正式发布结论：

- 阶段 0–4 任一阶段仍为 `FAIL`，或条件放行项已经超过批准期限；
- 英文或中文核心端到端流程存在失败；
- 任意正式支持平台的录音停止、保存、托盘或通知存在阻断故障；
- P0、P1 或未获书面批准的 P2 缺陷未关闭；
- 中文语言审校、隐私/法律文案审核或技术审核缺少签字；
- 安装、覆盖升级、卸载、用户数据保留或版本回滚测试失败；
- 发布包版本号、Git 提交、文件哈希和审计报告无法相互对应；
- 中文资源损坏时不能自动回退英文，或资源缺失会阻止应用启动；
- 缺少平台矩阵、测试日志、截图基线、已知问题或回滚手册中的任一必备证据。

#### 阶段 5 放行标准（发布门禁）

最终发布必须同时满足：

- 阶段 0–4 的验收结论均为 `PASS`，或仅有批准的非阻断 `CONDITIONAL PASS`；
- 所有 JSON、Schema、键集合、占位符和硬编码检查通过；
- 英文和中文端到端核心流程通过率 100%；
- 当前正式支持平台的必测矩阵通过；
- P0 = 0，P1 = 0，P2 = 0；
- P3 已登记负责人和计划版本；
- 中文语言审校、隐私/法律文案审核和技术审核均签字；
- 发布候选包完成安装、覆盖升级、卸载和回滚测试；
- 发布包哈希、版本号、构建提交和审计报告相互对应；
- 已验证英文应急回退，不会因中文资源问题阻止应用启动。

#### 发布后观察标准

发布后的第一个观察周期至少跟踪：

- 启动失败率；
- 缺失翻译键次数；
- Locale 加载和回退次数；
- 原生托盘重建失败次数；
- 结构化错误码中的未知错误比例；
- 中文用户的录音、转录、摘要和导入失败率；
- 用户反馈中的术语、乱码、截断和语义问题。

达到以下任一条件应停止扩大分发并评估回滚：

- 出现语言切换导致的数据丢失；
- 中文化导致录音无法停止或文件无法保存；
- 应用因 Locale 资源损坏无法启动；
- 隐私或录音同意文案存在实质性错误；
- 中文版本核心流程失败率显著高于英文基线；
- 出现 Secret、内部路径或会议敏感内容泄露。

#### 阶段 5 实际执行记录（2026-08-23）

| 验收域 | 实际结果 | 证据／备注 |
|---|---|---|
| 多语言资源 | PASS | 前端 13 Namespace、1578 键／语言；原生 33 键／语言；键、空值、占位符通过 |
| 模板内容 | PASS | 104/104 冻结候选解析，6 个内置模板中英文运行时可用 |
| 自动回归 | PASS | i18n 54/54；Node 兼容前端库 41/41；TypeScript 0；Next 13/13；Rust 278/0/2 |
| 运行时／冒泡 | PASS | 16/16；50 次切换；转录、模型、录音状态未改变；离线资源可用 |
| 视觉自动审计 | PASS | WebView2 模拟 100%/125%/150%/200%，0 乱码、0 原始键、0 无名称、0 横向裁切 |
| Windows 隔离安装 | PASS | 17/17；新装、启动、同版本修复、默认卸载、数据保留与测试清理 |
| 正式产物保护 | PASS | 正式 EXE 与正式安装包 SHA-256 均保持冻结值 |
| 人工／设备／治理门禁 | FAIL | 上游条件项、签字、签名、平台范围、60 分钟录音、真实 Provider、真实升级／回滚、读屏、原生 Shell 证据、冷启动基线与干净提交追溯未完成 |

阶段 5 的自动化技术套件为 `PASS`，但正式发布门禁按本节阻断规则判定为 **`FAIL／不得正式发布`**。隔离候选包仅用于审计，未复制到 `target/release`。完整结论、阻断项和签字区见：

- `docs/i18n/phase-5/phase-5-final-release-audit.md`；
- `docs/i18n/phase-5/end-to-end-regression-report.md`；
- `docs/i18n/phase-5/visual-accessibility-report.md`；
- `docs/i18n/phase-5/windows-install-upgrade-uninstall-report.md`；
- `docs/i18n/phase-5/performance-stability-report.md`；
- `docs/i18n/phase-5/platform-test-matrix.md`；
- `docs/i18n/phase-5/privacy-legal-review.md`；
- `docs/i18n/phase-5/known-issues-and-risk-acceptance.md`；
- `docs/i18n/phase-5/rollback-runbook.md`。

### 15.9 阶段 5A-1：发布源码整理与干净 RC 准备

执行日期：2026-08-23
执行状态：**PASS（5A-1 发布源码整理）／干净提交已验证，正式 RC 尚未生成**

本阶段把阶段 5 的 `P5-REL-001` 拆为可审计的源码整理任务。启动时 Git 状态包含 171 个正常条目；文件级展开得到 216,215 个未跟踪文件，其中 215,373 个为隔离构建、恢复缓存、本机工具或审计压缩包。通过 `.gitignore` 加固，这些本地产物已移出发布源码视图但没有删除。

逐文件机器清单已完成，未分类路径为 0。生产源码、构建、测试和仓库清理按 201 个文件规划；审计工具与架构文档实际按 97 个文件规划，其中 2 项自描述 JSON 为避免递归不计入 958 项机器清单；660 个历史生成证据独立归档；2 个纯换行／无内容状态差异排除。计划纳入发布源码共 298 个文件，5 个混合、安全敏感或配套行为文件保留为显式审核项。

敏感检查扫描约 30.7 MB 文本，强特征 Secret 为 0，凭据样式赋值的人工复核剩余为 0；包含本机路径的生成证据不进入生产源码提交。回归复跑结果为 i18n 54/54、模板／导航 41/41、TypeScript 0、Next 13/13、Rust 278/0/2。

已创建 `codex/meetily-i18n-release-source`，并按仓库清理、依赖构建、配套修复、前端 i18n、原生模板、测试、审计工具和文档八组边界提交；非中文化的 `package.json --turbo` Hunk 已排除。第一次干净验收发现 2E／2F 测试依赖未提交生成证据，已由 `12f5f8c` 改为临时目录自生成并清理。全新 R2 worktree 在安装前后和全部验证后均为 Git 状态 0，结果为 i18n 54/54、模板／导航 41/41、TypeScript 0、Next 13/13、Rust 278/0/2 加 Doc-test 1/1。Rust 测试通过仅测试期 `TAURI_CONFIG` 排除 sidecar 校验；正式打包仍必须提供真实 `llama-helper`。收尾期间另一个共享工作区任务继续修改模板功能，原工作树后来扩展到 120 个快照外跟踪状态项；这些变化未进入已验证提交，`c051ae2` 与 `d5669ae` 已把候选集合和纳入文件哈希固定到指定 Git Ref，避免并发工作树污染追溯。尚未生成并绑定新 RC，正式二进制也未覆盖，故仍不能关闭 `P5-REL-001`。详细证据：

- `docs/i18n/phase-5a1/release-source-inclusion-plan.md`；
- `docs/i18n/phase-5a1/release-source-exclusion-plan.md`；
- `docs/i18n/phase-5a1/commit-split-plan.md`；
- `docs/i18n/audit/phase-5a1-source/source-inventory.json`；
- `docs/i18n/audit/phase-5a1-source/sensitive-content-audit.json`；
- `docs/i18n/audit/phase-5a1-source/verification-summary.json`；
- `docs/i18n/audit/phase-5a1-source/phase5a1-audit-report.md`。

### 15.10 阶段 5A-2：批准变更合并、干净 RC 与安装链路复验

- 执行日期：2026-08-23
- 执行状态：**PASS（自动化技术与隔离 RC）／FAIL（生产发布 NO-GO）**

阶段 5A-2 从 5A-1 已验证基线 `f2eb887bfd0848803c4287d4f2da07f27df111a5` 建立独立集成工作树。共享工作区中的会议模板 JSON 导入／导出目标集共 10 个文件，经间隔 8 秒的双哈希采样确认 10/10 稳定；约 110 个并发 Rust 格式化漂移没有进入本阶段。隔离工作树的 10 个目标文件与冻结输入规范化字节 10/10 一致，功能提交为 `ea4b74fafd41f5932f3c82f410a447685b133e60`。

模板 JSON 方案已验证 1 MiB 单文件上限、UTF-8／BOM、无原文泄露的行列语法错误、V1 → V2 迁移、V2 Schema 和业务校验、原子持久化、符号链接目标拒绝、逐文件错误隔离、单批 50 文件上限、前端只显示文件基本名，以及英文／简体中文资源对等。

工具链提交 `2f274376a4b772b04dc0919666806714d0cec3e6` 已把两个 `bun:test` 测试迁移到 Node，建立 `test:unit`、`test:all` 和非交互 `lint`。最终 Node 单元测试 51/51、i18n 54/54，总计 105/105；ESLint 0 error／0 warning；TypeScript 0；Next 13/13；Rust 283 passed／0 failed／2 ignored，Doc-test 1/1。因此 `P5-TOOL-001` 与 `P5-TOOL-002` 已关闭。

可追溯 RC 从干净提交 `4edc53116b5d5e186d306dccdc5b45729777f487` 构建。构建前后已跟踪和未忽略未跟踪 Git 状态均为 0；841 个构建输入无文件晚于候选 EXE。`llama-helper` 从当前源码以 `--locked --release` 构建并通过 `ping/pong`、`shutdown/goodbye` 协议烟雾测试；FFmpeg 版本为 8.0.1。构建明确使用 `--no-sign --ci`，因此所有候选签名状态均如实记录为 `NotSigned`。

| 5A-2 产物／输入 | SHA-256 | 审计状态 |
|---|---|---|
| RC EXE | `D4C8E31D9504B25DC8C77DC30C0C5A506055E2C20BE082347B763901AA640D84` | NotSigned、仅审计 |
| RC NSIS 安装包 | `5CFE9C41360EC135C407FF8FC8D00EE23BAE46CB2E1E8632DAC51EE91BA61FBE` | NotSigned、仅审计 |
| `llama-helper` | `EE4E303B64603DFF8369644EDF3792ADD1948605C62F3BEC66A2549757EF97EC` | 当前源码构建、协议 PASS |
| FFmpeg | `5AF82A0D4FE2B9EAE211B967332EA97EDFC51C6B328CA35B827E73EAC560DC0D` | 8.0.1、Payload 哈希匹配 |
| 正式冻结 EXE | `1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823` | 未覆盖 |
| 正式冻结安装包 | `C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434` | 未覆盖 |

Windows 安装审计使用独立产品名 `Meetily Phase 5A2 RC`、标识 `com.meetily.ai.phase5a2rc` 和独立安装／AppData／注册表路径。全新安装、启动、同版本修复、sidecar Payload、默认卸载、数据保留和 finally 清理共 19/19 断言通过。随后使用同一隔离身份构建合成 0.3.9：原位升级到 0.4.0 返回 0，数据哨兵哈希不变且升级后可启动；尝试静默降级返回预期退出码 3，注册版本仍为 0.4.0，Payload EXE 与数据均未改变；升级版卸载和隔离清理成功，共 21/21 断言通过。

#### 5A-2 分部分验收审计标准

| 审计部分 | 强制通过标准 | 实际结果 | 判定 |
|---|---|---|---|
| 并发输入冻结 | 目标文件连续采样稳定，非目标漂移 0 混入 | 10/10 稳定；约 110 个格式化漂移排除 | PASS |
| 功能安全 | 大小、编码、版本迁移、Schema、原子写入、路径与错误隐私全部通过 | TypeScript 4/4，新增 Rust 场景全部通过 | PASS |
| 多语言 | en／zh-CN 键、占位符和可见反馈对等 | i18n 54/54 | PASS |
| Node 工具链 | 全部前端单元测试单一命令可执行 | 51/51；连同 i18n 为 105/105 | PASS |
| Lint | 非交互、0 error、0 warning；发布关键路径严格 | 0／0 | PASS |
| 类型与构建 | TypeScript 0，Next 全页面，Rust 0 失败，NSIS Exit 0 | 0；13/13；283/0/2；1/1 bundle | PASS |
| 源码追溯 | 构建提交干净，候选、提交、输入和 sidecar 可双向对应 | `4edc531`，前后状态 0，841/841 输入 | PASS |
| 全新安装 | 安装、启动、Payload、修复、卸载、保留与安全清理全部成功 | 19/19 | PASS |
| 安装器升级语义 | 旧版安装、原位升级、数据保留、降级阻止、卸载全部成功 | 21/21 | PASS |
| 正式产物保护 | 正式 EXE／安装包哈希与冻结基线一致 | 两项均未变化 | PASS |
| 真实历史数据迁移 | 批准的历史安装包＋脱敏旧数据升级／回滚后内容可读 | 未执行 | OPEN／阻断 |
| 生产代码签名 | Authenticode、时间戳、证书链和 SmartScreen 验证通过 | NotSigned | OPEN／阻断 |
| 治理与真机 | 签字、平台、60 分钟录音、真实 Provider、性能、读屏、Shell 证据齐全 | 未全部完成 | OPEN／阻断 |

#### 5A-2 缺陷状态变化

- `P5-REL-001`：**CLOSED**。干净提交 `4edc531` 已生成唯一可追溯 RC；
- `P5-TOOL-001`：**CLOSED**。Bun 依赖测试已迁移，Node 51/51；
- `P5-TOOL-002`：**CLOSED**。非交互 ESLint 0 error／0 warning；
- `P5-UPG-001`：**OPEN**。合成夹具只证明安装器语义，机器报告明确记录 `closesHistoricalDataMigrationGate=false`；
- `P5-SIGN-001`：**OPEN**。所有候选仍为 `NotSigned`；
- 其余阶段 5 治理、平台、硬件、AI、性能、可访问性和原生 Shell 门禁继续 OPEN。

5A-2 的自动化和隔离 RC 子阶段可判定为 **PASS**，但依据 15.8 的发布阻断规则，生产正式发布仍为 **FAIL／NO-GO**。不得把未签名审计候选复制为正式发布包，不得覆盖 `target/release`。详细证据：

- `docs/i18n/phase-5a2/phase-5a2-release-source-and-rc-audit.md`；
- `docs/i18n/phase-5a2/release-candidate-manifest.json`；
- `docs/i18n/phase-5a2/windows-install-upgrade-uninstall-report.md`；
- `docs/i18n/audit/phase-5a2/windows-install-audit.json`；
- `docs/i18n/audit/phase-5a2/windows-upgrade-downgrade-audit.json`。

### 15.11 阶段 5A-3：官方历史版本升级、语义数据保留与受控回滚审计

- 执行日期：2026-08-23 至 2026-08-24
- 执行状态：**PARTIAL PASS（历史来源、脱敏语义数据、原身份升级与受控回滚）／FAIL（直接降级防护与官方运行时迁移证明）／生产发布 NO-GO**

阶段 5A-3 使用实际产品身份 `meetily` / `com.meetily.ai` 建立了与官方 v0.3.0 的升级链路。官方 GitHub Release 的 v0.3.0 NSIS 安装包 SHA-256 为 `900C10A4EA05D991A06AB670886DE45DE08904903298A862A94FDC66F25420D9`，Authenticode 状态为 `Valid`，签名主体为 Zackriya Solutions Private Limited；安装后官方 EXE SHA-256 为 `0C1A60560490CF78BC9C14303A1598DB0F05DC887A7E8EB7BD00B7EC04408045`。v0.3.0 Git 标签是指向提交 `91b0c0985932d0797e249033601afa14f22ee3d3` 的轻量标签，不存在可独立验证的 tag 签名；该限制已显式记录，没有以 GitHub Release 资产签名替代 tag 签名声明。脱离式更新签名已使用配置的 Ed25519/minisign 公钥通过验证。

沙箱网络全程禁用，WebView2 使用 Microsoft 官方 x64 Evergreen Standalone Installer，SHA-256 为 `82B2D8A7013E0C0EA15D48FF4742EE3778BA16BD8B7B4A47876645B3E48D4016`，Authenticode 为 `Valid`。候选版本为实际身份的 0.4.1 审计候选：独立 EXE SHA-256 `4A1B762599ED79108DCE6FA56EF41162E11AF91723B34CC688E3F32567C3555A`，NSIS 安装包 SHA-256 `AD6D1B4B702873CBECBAD9CADD4F7166BC2C505DE70ACFCA226E1F8C46EF79EC`；二者均为 `NotSigned`，仅允许进入一次性 Windows Sandbox，不得覆盖正式发布包。

脱敏历史夹具包含 3 场会议、5 段转录、2 份摘要、2 份分块、2 份会议备注、设置与转录设置，覆盖英文、中文和中英混排内容，包含确定性 WAV 录音、偏好、通知与数据保留哨兵，API Key 列全部为空。数据库依照冻结的 v0.3.0 迁移源执行 10 个 SQLx 兼容迁移后富化，SHA-256 为 `C27B000DCEF5B496004DDD7B73E32BBB1F703952B029B27182BF46C97C312F3A`。夹具类型已明确标记为 `synthetic-redacted-frozen-v030-source-migrated-and-enriched`，`runtimeMigrationClaimed=false`；未读取本机真实 Meetily 用户数据。

官方 v0.3.0 在多次沙箱探针中能安装、启动并在 `com.meetily.ai` 目录下发现数据文件，但对仅含初始 schema 的 seed 数据库未观察到运行时迁移：文件大小、SHA-256 和时间戳不变，也没有 WAL/SHM。CDP 运行时命令探针因远程调试端口不可达而失败，不作为通过证据。因此本阶段只证明“冻结历史迁移源兼容的脱敏数据”可升级与回滚，不宣称“官方 v0.3.0 二进制已自动执行历史迁移”。

最终升级矩阵从官方 0.3.0 原身份安装出发，恢复脱敏夹具，完成损坏包预执行哈希阻断、0.4.1 原位升级、候选启动、升级后快照、直接降级观测、安装器所有权清理、候选自身卸载、数据保留、升级前快照恢复和官方 0.3.0 重装／启动。受控回滚路径上，候选卸载后 1.015 秒内注册表和程序目录均删除，AppData 保留，官方 0.3.0 身份、签名、启动和受保护文件全部恢复。

直接运行旧版官方 0.3.0 安装包对 0.4.1 静默降级时，进程退出码为 0，注册版本变为 0.3.0，且安装目录留下 12 个 0.4.1 中英本地化模板文件。这是稳定复现的跨版本所有权污染，因此直接降级必须标记为 **UNSAFE／FAIL**；只能使用文档化的受控回滚流程。

独立候选 EXE 与 NSIS 安装负载 EXE 大小相同但哈希不同。字节级比较证明仅有偏移 `0x330042A` 起的 3 个连续字节不同：`UNK` 变为 `NSS`，上下文为 `__TAURI_BUNDLE_TYPE_VAR_*`。锁定的 `tauri-utils 2.9.1` 源码确认 `UNK` 是待打包修补的默认值，`NSS` 映射 `BundleType::Nsis`。因此 NSIS 负载哈希 `4B35A0398F49A0CC514791888D8857A2D9F576794ECC8120B401D4FDC419DFBD` 是可重复的 Tauri bundle-type 修补结果，不是第二次产品编译。

前端门禁复验为 ESLint 0 error／0 warning、Node 单元测试 51/51、i18n 54/54、TypeScript 0、Next 13/13。Rust 首次全量复验在与 Sandbox 重负载并发时，1000 模板列表 P95 为 682.8108 ms，超过 500 ms 预算；该失败未删除。隔离后连续三轮 P95 为 292.6903、270.2783 和 221.1673 ms，随后全量 `cargo test --release -p meetily` 为 283 passed／0 failed／2 ignored，Doc-test 1/1。该用例判定为资源竞争敏感，500 ms 门槛仍保留。

#### 5A-3 分部分验收审计标准

| 审计部分 | 强制通过标准 | 实际结果 | 判定 |
|---|---|---|---|
| 历史来源 | Release API digest、Authenticode、安装后 EXE、tag 提交与更新签名均可追溯 | 资产哈希与 API digest 一致，官方签名有效；轻量 tag 限制显式记录 | PASS |
| 离线前提 | Sandbox 断网，WebView2 官方离线包哈希与签名有效 | 断网安装 151.0.4129.101，签名有效 | PASS |
| 脱敏夹具 | 无真实用户数据、无 Secret，中英文、DB、录音、设置和哨兵齐全 | 3 会议，API Key 全空，夹具清单哈希固定 | PASS |
| 官方运行时迁移 | 官方 v0.3.0 二进制对旧 seed 执行迁移，DB 变化可证明 | 多轮未观察到 DB 变化；CDP 不可达 | FAIL／阻断 |
| 冻结源迁移兼容 | v0.3.0 的 10 个迁移文件与当前源一致，SQLx 校验和表结构可复现 | 10/10 blob 一致，10 条 `_sqlx_migrations` 校验通过 | PASS |
| 候选可追溯性 | 实际身份、干净来源、候选与 sidecar 哈希固定 | 0.4.1 EXE、NSIS、NSIS 负载、FFmpeg、helper 全部固定 | PASS（仅审计） |
| 损坏包保护 | 被翻转字节的安装包在执行前因哈希不匹配被阻断 | 损坏哈希不同，未启动可执行文件 | PASS |
| 原位升级 | 官方 0.3.0 → 0.4.1，注册版本、负载哈希和启动均正确 | Exit 0，0.4.1，负载哈希匹配，15 秒存活 | PASS |
| 语义数据保留 | 升级前／后 DB integrity、schema、全行、文件与用户通知设置语义相等 | 10/10 语义断言通过，DB 文件哈希未变 | PASS |
| 直接降级保护 | 旧版安装包不得静默覆盖新版，不得留下跨版本资源 | 降级 Exit 0，留下 12 个 0.4.1 本地化模板 | FAIL／阻断 |
| 受控回滚 | 按所有权顺序卸载，保留 AppData，恢复快照和官方 0.3.0 | 程序目录 1.015 秒清除，官方身份／启动／数据全恢复 | PASS |
| 二进制一致性 | 独立 EXE 与安装负载差异可解释、可重复、无产品码漂移 | 仅 `UNK` → `NSS` 3 字节 Tauri 标记 | PASS |
| 全量回归 | Lint 0/0，Node/i18n/TS/Next/Rust/Doc-test 全通过，性能 P95 < 500 ms | 0/0；51/51；54/54；0；13/13；283/0/2；1/1；隔离三轮均达标 | PASS |
| 真实历史数据 | 经授权备份的真实旧数据升级／回滚语义通过 | 未取得授权样本，未读取本机真实数据 | OPEN／阻断 |
| 生产签名 | EXE/NSIS 签名、时间戳和证书链通过 | 0.4.1 候选为 NotSigned | OPEN／阻断 |
| 正式产物保护 | `target/release` 既有 EXE/NSIS 哈希不变 | `1873…57823` / `C6A5…30434` 均不变 | PASS |

#### 5A-3 缺陷与门禁状态

- `P5-UPG-SCHEMA-001`：**CLOSED**。冻结 v0.3.0 迁移源兼容的脱敏 DB 在升级和受控回滚后语义 10/10 通过；
- `P5-UPG-RUNTIME-MIGRATION-001`：**OPEN／阻断**。官方 v0.3.0 二进制未在 seed DB 上留下可验证迁移证据；
- `P5-UPG-REALDATA-001`：**OPEN／阻断**。未使用经授权的真实历史备份；
- `P5-DOWNGRADE-001`：**OPEN／阻断**。直接静默降级成功且留下 12 个新版模板，必须阻止或强制受控回滚；
- `P5-ROLLBACK-001`：**CLOSED（脱敏夹具）**。所有权顺序清理、快照恢复、官方重装和语义等价均通过；
- `P5-PKG-MARKER-001`：**CLOSED**。独立 EXE 与 NSIS 负载的哈希差异已定位为 Tauri `UNK` → `NSS` 三字节修补；
- `P5-PERF-001`：**CLOSED（当前候选）**。首次并发失败保留，三轮隔离复测与全量复测通过；
- `P5-SIGN-001`：**OPEN／阻断**。实际身份 0.4.1 候选仍为 `NotSigned`；
- 阶段 5 治理签字、多真机／平台、60 分钟录音、真实 Provider、读屏与原生 Shell 门禁继续 OPEN。

因 `P5-UPG-RUNTIME-MIGRATION-001`、`P5-UPG-REALDATA-001`、`P5-DOWNGRADE-001` 和 `P5-SIGN-001` 仍为阻断项，阶段 5A-3 不得宣布生产发布通过；结论为 **FAIL／NO-GO**。正式冻结 EXE 与 NSIS 未被覆盖。详细证据：

- `docs/i18n/phase-5a3/phase-5a3-historical-upgrade-audit.md`；
- `docs/i18n/phase-5a3/candidate-v041-manifest.json`；
- `docs/i18n/audit/phase-5a3/official-v030-provenance-audit.json`；
- `docs/i18n/audit/phase-5a3/official-v030-updater-signature-audit.json`；
- `docs/i18n/audit/phase-5a3/historical-v030-enriched-fixture-manifest.json`；
- `docs/i18n/audit/phase-5a3/official-v030-to-v041-upgrade-audit.json`；
- `docs/i18n/audit/phase-5a3/upgrade-semantic-equivalence-audit.json`；
- `docs/i18n/audit/phase-5a3/candidate-executable-bundle-marker-audit.json`；
- `docs/i18n/audit/phase-5a3/rust-template-performance-rerun-audit.json`；
- `docs/i18n/audit/phase-5a3/official-v030-seed-migration-not-observed.json`。

### 15.12 阶段 5A-4A：受控回滚与安装资源所有权隔离

- 执行日期：2026-08-24
- 执行状态：**PASS（5A-4A 技术子阶段）／FAIL（阶段 5 生产发布 NO-GO）**

阶段 5A-4A 已把版本变更拆成三条强制通道：应用内更新器只接受严格更高的完整 SemVer，并在发现和安装前各校验一次；当前 NSIS 安装器同时使用 `allowDowngrades=false` 和 `NSIS_HOOK_PREINSTALL`，在交互及 `/S` 模式下重新比较注册版本，旧版本以非零退出；确需降级时只允许使用受控回滚工具，且必须提供明确确认、目标／恢复安装包 SHA-256、默认有效 Authenticode、目标版本兼容快照、当前版本紧急快照和结构化审计报告。

版本化所有权清单 `install-resource-ownership.v1.json` 冻结了 0.3.0 的 6 个旧版根模板以及 0.4.x 的 18 个模板残留（6 个继承根模板＋6 个英文＋6 个中文），每项都绑定路径和 SHA-256。清理采用 fail-closed：未知文件、哈希不符、未知目录、重解析点或非空安装根都会在删除前阻断；目录只在为空时删除。`%APPDATA%/com.meetily.ai`、自定义模板仓库、配置目录和默认录音目录是独立受保护数据根，不属于安装资源清理范围。

回滚工具会先执行只读预检，再创建当前版本紧急数据快照；随后卸载当前版本、按精确所有权清理、恢复并验证目标版本兼容快照、安装目标版本。任何变更阶段失败时会尽力移除部分目标、恢复紧急数据并重装冻结的当前版本恢复包。第一次真实回滚因清单未包含六个继承根模板而在 `templates/daily_standup.json` 处正确 fail-closed，文件未删除，紧急数据恢复和 0.4.1 重装成功，`recoveredAfterFailure=true`；失败证据已保留，清单修正后才重跑最终矩阵。

最终断网 Windows Sandbox 从运行中的 0.4.1 审计候选回滚到官方 0.3.0：Preflight `passed=true` 且 `mutated=false`，Rollback `passed=true`，官方目标安装包 SHA-256 为 `900C10A4EA05D991A06AB670886DE45DE08904903298A862A94FDC66F25420D9`、Authenticode `Valid`；目标数据逐文件恢复，安装后 EXE SHA-256 为 `0C1A60560490CF78BC9C14303A1598DB0F05DC887A7E8EB7BD00B7EC04408045`、签名 `Valid`、本地化跨版本残留 0、启动存活 12 秒。最终六项 Sandbox 断言全部为 `true`。

自动门禁结果为：静态审计 15/15；PowerShell 5.1 与 7 失败注入均 10/10；ESLint 0/0；Node 55/55；i18n 54/54；TypeScript Exit 0；Next 13/13；Rust 283 passed／0 failed／2 ignored。Rust 旧缓存测试 EXE 缺少 Common Controls v6 manifest 和首次受支持目标因 FFmpeg 不在 PATH 的失败均保留；最终只向测试进程暴露冻结 sidecar，SHA-256 `5AF82A0D4FE2B9EAE211B967332EA97EDFC51C6B328CA35B827E73EAC560DC0D`，未修改产品代码掩盖环境问题。

#### 5A-4A 分部分验收审计标准

| 审计部分 | 强制通过标准 | 实际结果 | 判定 |
|---|---|---|---|
| 更新器门禁 | 发现和安装前只接受严格更高 SemVer；无效、相等、较旧都拒绝 | 双重门禁和 4 项 SemVer 单元测试通过 | PASS |
| 当前安装器门禁 | 交互和 `/S` 模式都阻止降级并返回非零 | Hook、SemverCompare、Silent、Exit 3 静态审计通过 | PASS（源码／静态） |
| 安装资源所有权 | 各版本所有残留有安全相对路径和精确哈希 | 0.3.0 = 6；0.4.x = 18；源哈希全匹配 | PASS |
| Fail-closed 清理 | 未知、哈希不符、重解析点、未知目录均在删除前阻断 | 单元失败注入和真实未知根模板触发均通过 | PASS |
| 数据快照 | 产品／版本／根映射／文件哈希和大小完整，重复根／文件声明阻断，恢复后复验 | 正常往返、损坏快照与重复声明阻断通过 | PASS |
| 失败恢复 | 当前数据在变更前备份；失败后数据恢复并重装当前版本 | 第一次真实回滚故障中成功触发 | PASS |
| 真实受控回滚 | 0.4.1 → 官方 0.3.0；数据、资源、身份、签名、启动全部通过 | 六项最终 Sandbox 断言全部为 true | PASS |
| 全量回归 | 静态、PowerShell、前端、i18n、TS、Next、Rust 无失败 | 15/15；10/10×2；0/0；55/55；54/54；0；13/13；283/0/2 | PASS |
| 正式产物保护 | 既有正式 EXE／NSIS 哈希不变 | `1873…57823`／`C6A5…30434` | PASS |
| 生产签名 | 正式候选和恢复包的签名、时间戳、证书链有效 | 0.4.1 审计候选 NotSigned | OPEN／阻断 |
| 真实历史数据 | 经授权真实旧数据完成升级与回滚语义审计 | 未读取真实数据 | OPEN／阻断 |

#### 5A-4A 缺陷与门禁状态

- `P5-DOWNGRADE-001`：**CLOSED（受支持通道）／历史安装器旁路 PROHIBITED**。更新器和当前安装器已禁止降级，受控回滚路径通过；已经发布的旧安装器不能被追溯修改，直接手工执行仍不受支持；
- `P5-INSTALL-OWNERSHIP-001`：**CLOSED**。0.3.0／0.4.x 精确所有权清单建立，最终跨版本本地化残留为 0；
- `P5-ROLLBACK-RECOVERY-001`：**CLOSED（脱敏／Sandbox）**。真实未知残留触发后自动恢复当前数据和版本成功；
- `P5-UPG-RUNTIME-MIGRATION-001`：**OPEN／阻断**；
- `P5-UPG-REALDATA-001`：**OPEN／阻断**；
- `P5-SIGN-001`：**OPEN／阻断**；
- 阶段 5 真机、平台、60 分钟录音、真实 Provider、读屏、原生 Shell 和治理签字继续 OPEN。

5A-4A 可以进入下一技术子阶段，但生产发布仍是 **FAIL／NO-GO**。下一优先项为 **5A-4B：生产签名链与受控回滚包签名／时间戳验证**，之后处理真实历史数据和官方运行时迁移证明。详细证据：

- `docs/i18n/phase-5a4/phase-5a4a-controlled-rollback-audit.md`；
- `docs/i18n/phase-5a4/phase-5a4a-controlled-rollback-runbook.zh-CN.md`；
- `docs/i18n/phase-5a4/install-resource-ownership.v1.json`；
- `docs/i18n/audit/phase-5a4/phase-5a4a-static-audit.json`；
- `docs/i18n/audit/phase-5a4/controlled-rollback-sandbox-final.json`；
- `docs/i18n/audit/phase-5a4/controlled-rollback-tool-final.json`；
- `docs/i18n/audit/phase-5a4/regression-gates-final.json`；
- `docs/i18n/audit/phase-5a4/failure-history.json`。

### 15.13 阶段 5A-4B：生产签名链、时间戳与更新签名门禁

- 执行日期：2026-08-24
- 执行状态：**PASS（签名架构与技术门禁）／FAIL（真实生产签名未执行，阶段 5 发布 NO-GO）**

阶段 5A-4B 关闭了原 Tauri Windows 签名脚本在 `DIGICERT_KEYPAIR_ALIAS` 缺失时 `exit 0` 的 Critical 短路路径。生产模式现在默认 fail-closed：必须存在获批的发布者主体、角色化证书指纹、代码签名 EKU、Windows Default Authenticode 信任链、受信时间戳链、DigiCert／KeyLocker 环境和 Tauri 更新私钥。签名后不会只看 `Status=Valid`，还会运行 Windows SDK SignTool `/pa /all /tw /u` 并按角色比较主体和指纹。

新策略把当前发布包、回滚恢复包和上游历史目标包分成 `ProductionArtifact`、`RecoveryInstaller`、`HistoricalRollbackTarget` 三个角色。历史 0.3.0 证书指纹只进入 `upstreamHistorical`，不能为当前生产发布授权；`productionActive` 有意为空，因此没有发布负责人批准当前证书前，正式构建会失败。

三条 Windows CI 路径已移除 API Key 前缀、KeyLocker 列表和别名值日志；Keypair Alias 改为受保护 Secret；客户端认证 P12 只写入 `RUNNER_TEMP` 并在工作流结束时安全删除。正式 release 工作流固定 `sign-binaries: true`。开发无签名构建只能显式使用 `AuditUnsigned` 和完整确认词，Tag／Release 事件以及生产回滚均拒绝该旁路。

Tauri 更新签名新增独立密码学复验：逐产物验证 BLAKE2b-512、Ed25519 主签名、可信注释签名和公钥 ID `ECA631D78797C82A`。上游官方 0.3.0 的 Authenticode、代码签名 EKU、DigiCert 时间戳和 Tauri 双签名均通过；当前 0.4.1 审计候选 `NotSigned` 且没有 `.sig`，生产发布门禁按预期失败；对临时副本追加 1 字节后，Authenticode 与更新主签名均按预期失败。

最终自动证据：签名静态门禁 34/34；PowerShell 7 与 Windows PowerShell 5.1 各 18/18；工作流 YAML 4/4；原 5A-4A PowerShell 回归 10/10 × 2；5A-4A 静态回归 15/15；完整断网 Windows Sandbox 六项断言全真；前端单元 55/55；i18n 54/54；最终聚合 14/14。Sandbox 使用显式 Audit 模式验证未签名审计候选的功能回滚，不构成生产签名证据。

#### 5A-4B 分部分验收审计标准

| 审计部分 | 强制通过标准 | 实际结果 | 判定 |
|---|---|---|---|
| 策略冻结 | 算法、主体、指纹集合、角色、EKU、时间戳和更新 Key ID 可审查 | v1 策略已冻结 | PASS |
| 生产默认拒绝 | 缺凭据、空生产集合、错指纹或签名失败均非零 | 双 PowerShell 与子进程负向测试通过 | PASS |
| 证书身份 | 主体与角色指纹同时匹配 | 上游历史角色正样本通过，生产角色锁定 | PASS |
| 信任链／时间戳 | SignTool `/pa /all /tw /u` Exit 0 | 上游真实样本 Exit 0；缺失分支拒绝 | PASS |
| Tauri 更新签名 | 主签名、可信注释和 Key ID 全部通过 | 正样本通过、篡改样本失败 | PASS |
| CI 保密 | 不输出 Secret 值、前缀、KeyLocker 列表或别名 | 34 项静态门禁通过 | PASS |
| 审计模式隔离 | 明确模式／确认词；Tag、Release、生产回滚禁止旁路 | 负向测试通过 | PASS |
| 回滚角色 | 历史目标与当前恢复包使用独立指纹集合 | 已接入受控回滚 | PASS |
| 5A-4A 回归 | 单元、静态和真实 Sandbox 无回归 | 10×2、15、6 项全部通过 | PASS |
| 正式产物保护 | 既有正式 EXE／NSIS 哈希不变 | `1873…57823`／`C6A5…30434` | PASS |
| 真实生产证书 | 获批主体／指纹、DigiCert 凭据和更新私钥可用 | 当前不可用 | OPEN／阻断 |
| 真实签名中文 RC | EXE、NSIS、`.sig`、`latest.json` 和安装／回滚全部通过 | 未生成 | OPEN／阻断 |

#### 5A-4B 缺陷与门禁状态

- `P5-SIGN-SKIP-001`：**CLOSED**；
- `P5-SIGN-POLICY-002`：**CLOSED**；
- `P5-SIGN-LOG-003`：**CLOSED**；
- `P5-SIGN-CONTINUE-004`：**CLOSED**；
- `P5-UPDATER-VERIFY-005`：**CLOSED**；
- `P5-ROLLBACK-MODE-006`：**CLOSED**；
- `P5-SIGN-001`：**OPEN／阻断**，等待受控批准当前发行证书并执行真实签名 RC；
- `P5-UPG-RUNTIME-MIGRATION-001` 与 `P5-UPG-REALDATA-001`：继续 **OPEN／阻断**。

5A-4B 技术子阶段可以验收，但生产发布继续为 **FAIL／NO-GO**。下一技术子阶段为 **5A-4C：受控批准当前发布证书并执行真实签名 RC 门禁**；它需要合法的发布者主体／证书指纹、DigiCert/SMCTL 凭据和 Tauri 更新私钥，不能用自签名、上游历史签名或模拟证据替代。详细证据：

- `docs/i18n/phase-5a4/phase-5a4b-production-signing-audit.md`；
- `docs/i18n/phase-5a4/phase-5a4b-production-signing-runbook.zh-CN.md`；
- `docs/i18n/phase-5a4/windows-signing-policy.v1.json`；
- `docs/i18n/scripts/Meetily.Signing.psm1`；
- `docs/i18n/scripts/audit-phase5a4b-release-signing.ps1`；
- `docs/i18n/scripts/verify-tauri-updater-signature.mjs`；
- `docs/i18n/scripts/test-phase5a4b-signing-policy.ps1`；
- `docs/i18n/scripts/audit-phase5a4b-production-signing.mjs`；
- `docs/i18n/scripts/capture-phase5a4b-final-evidence.mjs`。

### 15.14 阶段 5A-4C-1：Windows 生产证书准入预审

- 执行日期：2026-08-24
- 执行状态：**PASS（准入控制实现）／FAIL（真实证书尚未获批，阶段 5 发布 NO-GO）**

阶段 5A-4C-1 新增独立 `windows-certificate-admission.v1.json`，把证书候选公开字段、唯一活动指纹、公共信任链、Code Signing EKU、至少 30 天剩余有效期、DigiCert 健康检查／证书同步、一次性 PE 持钥证明、Tauri 私钥入口以及发布负责人／安全复核人双审批冻结成可审查状态机。准入批准只允许进入真实签名 RC，不单独构成发布授权。

生产前置脚本现已在任何 DigiCert 调用前验证准入记录：状态不是 `Approved`、候选与唯一 `productionActive` 不一致、审批缺失或复用历史证书时立即失败。同步后再次核对证书主体、指纹、序列号、Issuer、EKU、在线撤销链、有效期和自签名状态。预审报告只输出环境入口存在性，不读取或输出 API Key、P12 密码、Keypair Alias 值或 Tauri 私钥材料。

自动验收结果为 PowerShell 7 32/32、Windows PowerShell 5.1 32/32、静态安全控制 48/48；5A-4B 真实签名样本回归 30/30、18/18 × 2；前端单元 55/55、i18n 54/54、ESLint Exit 0；秘密哨兵扫描为零；强制准入和生产准备脚本在未批准状态下均以 Exit 1 拒绝。正式便携 EXE／NSIS 的 SHA-256 仍为 `1873…57823`／`C6A5…30434`。

真实本机审计确认 SignTool 可用，但 `smctl`、DigiCert 六项环境入口、Tauri 两项私钥入口和候选同步证书均不可用；准入记录保持 `PendingCandidateEvidence`，两项审批保持 Pending，`productionActive` 保持为空。上述结果证明失败关闭有效，不是生产就绪证明。

#### 5A-4C-1 分部分验收审计标准

| 审计部分 | 强制通过标准 | 实际结果 | 判定 |
|---|---|---|---|
| 准入记录 | schema、目标、候选证据和审批可追踪 | 结构冻结，候选未提供 | 技术 PASS／准入阻断 |
| 身份和角色 | 唯一活动指纹、精确主体、禁止历史复用 | 负向测试通过；活动集合空 | 技术 PASS／准入阻断 |
| 证书质量 | 公共链、Code Signing EKU、有效期 ≥30 天、自签名禁止 | 逻辑测试通过；无真实候选 | 技术 PASS／准入阻断 |
| 双审批 | 发布负责人和安全复核人独立批准，带时间与引用 | 两项 Pending | 阻断 |
| DigiCert 通道 | 工具、环境、客户端证书、healthcheck、certsync 全通过 | 当前不可用 | 阻断 |
| 持钥证明 | 一次性 PE 真签、时间戳、SignTool 和候选身份一致 | 未执行 | 阻断 |
| Tauri 更新钥匙 | Key ID 匹配，私钥入口存在但内容不进入报告 | 公钥通过，私钥入口缺失 | 阻断 |
| 失败关闭 | 任一必需条件缺失时非零退出 | 两条真实负向门禁 Exit 1 | PASS |
| 秘密保护 | 日志／报告无秘密值、别名值和私钥材料 | 扫描为零 | PASS |
| 正式产物保护 | 既有正式 EXE／NSIS 哈希不变 | `1873…57823`／`C6A5…30434` | PASS |
| 真实签名 RC | Authenticode、Tauri `.sig`、`latest.json`、安装和回滚全通过 | 未生成 | 后续阻断 |

#### 5A-4C-1 缺陷与门禁状态

- `P5-CERT-ADMISSION-001`：**CLOSED（控制实现）／OPEN（真实候选）**；
- `P5-CERT-APPROVAL-002`：**OPEN／阻断**；
- `P5-CERT-POSSESSION-003`：**OPEN／阻断**；
- `P5-UPDATER-PRIVATEKEY-004`：**OPEN／阻断**；
- `P5-SIGN-001`：继续 **OPEN／阻断**；
- `P5-UPG-RUNTIME-MIGRATION-001` 与 `P5-UPG-REALDATA-001`：继续 **OPEN／阻断**。

5A-4C-1 的技术控制可以验收，但真实生产证书准入和阶段 5 发布继续为 **FAIL／NO-GO**。合法 DigiCert 通道可用后，先执行真实连接、同步和一次性 PE 持钥证明，再由两个独立角色审批候选并更新唯一 `productionActive`；随后进入 **5A-4C-2：构建并真实签名全新中文 RC**。不得用自签名、历史上游证书或合成证据替代。

详细证据：

- `docs/i18n/phase-5a4/phase-5a4c1-certificate-admission-audit.md`；
- `docs/i18n/phase-5a4/phase-5a4c1-certificate-admission-runbook.zh-CN.md`；
- `docs/i18n/phase-5a4/windows-certificate-admission.v1.json`；
- `docs/i18n/scripts/Meetily.CertificateAdmission.psm1`；
- `docs/i18n/scripts/audit-phase5a4c1-certificate-admission.ps1`；
- `docs/i18n/scripts/audit-phase5a4c1-certificate-admission.mjs`；
- `docs/i18n/scripts/test-phase5a4c1-certificate-admission.ps1`。

## 16. 自动化检查

建议添加以下 CI 检查：

### 16.1 JSON 与键集合

- 所有 JSON 必须可解析；
- `zh-CN` 与 `en` 键集合一致；
- 不允许中文包缺键；
- 不允许值为 `null` 或空字符串；
- `_meta` 不参与翻译键对比。

### 16.2 占位符一致性

英文和中文的占位符集合必须相同：

```text
en:    "Downloaded {{count}} models"
zh-CN: "已下载 {{count}} 个模型"
```

如果中文漏掉 `count`，CI 应失败。

### 16.3 硬编码扫描

在 `frontend/src` 中扫描新的 JSX 英文、Toast 和用户属性。允许列表只包括：

- 产品/模型品牌；
- API、文件扩展名和协议值；
- 开发日志；
- 测试 fixture；
- CSS 类名和代码示例。

本次交付的 `extract-i18n-candidates.mjs` 可作为初始审计脚本，但落地后应增加基于 AST 的差异模式：CI 只报告相对基线新增的候选，避免一次出现数百条历史告警。

### 16.4 测试

- Locale 解析：`zh-CN`、`zh-TW`、`en-US`、未知 Locale；
- 保存与重启：语言选择应持久化；
- 运行时切换：不丢失录音、模型或页面状态；
- 复数与插值；
- 缺失键回退到英文；
- `<html lang>` 和 `dir`；
- 托盘菜单切换；
- 原生错误码映射；
- 模板 Locale 与摘要语言独立。

## 17. 英文 JSON 基线的使用方式

本目录中的 `en.json` 是由当前源代码生成的迁移基线，作用是保证没有大面积漏项。它不是最终键名审校完成的生产包。

使用顺序：

1. 在 `en.catalog.json` 中按 `status=translate` 过滤；
2. 查看 `sources` 确认上下文和所有出现位置；
3. 将自动键归并成本文建议的语义 Namespace；
4. 将复杂源表达式改成简单占位符；
5. 迁移源代码调用；
6. 再创建对应 `zh-CN` 键并翻译；
7. 完成后将目录项状态从 `translate` 更新为 `migrated`；
8. 运行硬编码增量扫描。

例如自动基线键可能是：

```text
ui.components.recordingControls.startRecording
```

迁移时可以归并为：

```text
recording.actions.start
```

这种人工归并非常重要，否则语言包会按组件实现细节组织，组件重构时键名也会不断变化。

## 18. 完成定义

简体中文国际化可以认为完成，必须同时满足：

- 设置中可以选择跟随系统、英文和简体中文；
- 切换语言即时生效并在重启后保持；
- 核心流程没有可见英文硬编码；
- 托盘菜单和系统通知同步切换；
- 所有用户错误通过错误码翻译；
- 语言名称、日期、数字和相对时间本地化；
- `html.lang`、字体和布局正确；
- 转录语言、摘要语言和 UI Locale 完全独立；
- 内置模板按内容 Locale 管理；
- 英文永远是可靠回退语言；
- CI 检查键集合、占位符和新增硬编码；
- 英文和中文至少各完成一次端到端人工验收。
