# Meetily 原生错误契约与注册表

## 1. IPC Schema

所有阶段 3 新增的原生 Locale 命令，以及通知系统的 Tauri 命令失败，统一返回以下结构：

```json
{
  "code": "I18N_INVALID_LOCALE",
  "params": {},
  "debugMessage": "Rejected unsupported UI locale"
}
```

- `code`：稳定、可检索、不得翻译；
- `params`：仅允许无敏感信息的展示参数；
- `debugMessage`：经过清洗的固定英文诊断摘要，不得包含绝对路径、API Key、URL 查询参数、会议正文、Rust Debug 或堆栈；
- 原始底层错误只写 Rust 日志，绝不复制到 OS 通知或用户消息；
- 未注册 code 必须回退为 `NATIVE_UNKNOWN`，前端必须回退到 `common:errors.unknown`。

## 2. 当前注册表

| code | 原生资源 key | 安全 debugMessage | 使用边界 |
|---|---|---|---|
| `NATIVE_UNKNOWN` | `error.nativeUnknown` | `Unexpected native operation failure` | 未知错误兜底 |
| `I18N_INVALID_LOCALE` | `error.invalidLocale` | `Rejected unsupported UI locale` | Locale 输入校验 |
| `I18N_STATE_UNAVAILABLE` | `error.localeStateUnavailable` | `Native locale state was unavailable` | Locale 锁/状态失败 |
| `I18N_PERSISTENCE_FAILED` | `error.localePersistenceFailed` | `Native locale preference persistence failed` | Locale 持久化失败 |
| `NOTIFICATION_MANAGER_UNAVAILABLE` | `error.notificationManagerUnavailable` | `Notification manager was not initialized` | 通知管理器未就绪 |
| `NOTIFICATION_OPERATION_FAILED` | `error.notificationOperationFailed` | `Notification operation failed` | 通知操作失败 |
| `NOTIFICATION_SERIALIZATION_FAILED` | `error.notificationSerializationFailed` | `Notification statistics serialization failed` | 通知状态序列化失败 |

注册表权威源码是 `frontend/src-tauri/src/i18n/error.rs`；本文件是审计说明，不得反向作为运行时数据源。

## 3. 可见性政策

原 `en.catalog.json` 的 447 条在线原生候选不等于 447 条用户消息。阶段 3 最终处置矩阵逐条区分：托盘/通知直接可见文本、Tauri 命令边界错误、日志/诊断、机器值和旧代码。只有会到达用户界面的错误才允许映射成翻译；日志、协议值、文件格式、模型 ID 和 SQL 片段不得翻译。

旧的 `Result<T, String>` 只有在前端已经用稳定业务状态映射且原字符串不会渲染时才能保留；任何新命令或新直出路径必须使用 `NativeError`。如果后续把目前仅写日志/仅用于控制流的错误展示到 UI，必须先登记稳定 code，不能匹配自由英文。

## 4. 安全规则

- 系统错误通知固定显示安全通用正文；原始错误只记录日志；
- 转录完成通知只显示“已完成/已保存”，不显示文件系统路径；
- 未知 Locale 返回 `I18N_INVALID_LOCALE`，不回显用户提交的任意字符串；
- 前端错误解析器只信任完整 `{ code, params, debugMessage }` 结构；字符串、`Error` 对象和未知 code 均回退到通用消息。
