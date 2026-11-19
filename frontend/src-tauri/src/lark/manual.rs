use super::{app::{self, AppConfig, db_error, now}, payload::fingerprint};
use crate::{state::AppState, summary::{markdown_export, meeting_settings}};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Review {
    pub chat_id: String,
    pub title: String,
    pub meeting_time: String,
    pub participants: String,
    pub absent: String,
    pub host: String,
    pub summary: String,
    #[serde(default)]
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    #[serde(default)]
    pub document_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_template: Option<String>,
    #[serde(default)]
    pub include_markdown: bool,
    #[serde(default)]
    pub attendance_confirmed: bool,
    #[serde(default)]
    pub participants_edited: bool,
    #[serde(default)]
    pub absent_edited: bool,
}

fn valid_chat(id: &str) -> bool {
    id.starts_with("oc_") && id.len() > 3 && id.len() < 100 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}
fn config_revision(c: &AppConfig) -> String { fingerprint(&[&c.app_id, &c.secret, &c.enabled.to_string()]) }
async fn configured(pool: &SqlitePool) -> Result<AppConfig, String> {
    let c = app::load(pool).await?;
    if !c.enabled || c.app_id.is_empty() || c.secret.is_empty() { return Err("请在设置中导入会议助手凭证并启用会议助手。".into()); }
    Ok(c)
}
fn validate(review: &Review) -> Result<(), String> {
    if !review.attendance_confirmed { return Err("请核对出席、缺席人员，并勾选确认名单。".into()); }
    selected_document_ids(review)?;
    if !valid_chat(&review.chat_id) { return Err("请选择机器人已加入的目标群。".into()); }
    super::card::validate_message_edit(&review.title, &review.summary)?;
    if [&review.participants, &review.absent, &review.host].iter().any(|s| s.chars().count() > 1000) { return Err("人员名单过长。".into()); }
    parse_review_time(&review.meeting_time).map_err(|_| "请填写完整的会议日期和时间（北京时间）。")?;
    meeting_settings::validate_people(&review.participants, &review.absent)?;
    Ok(())
}
fn selected_document_ids(review: &Review) -> Result<Vec<String>, String> {
    let mut ids=Vec::new();
    for id in review.document_ids.iter().chain(review.document_id.iter()) {
        if id.trim().is_empty() || id.len()>128 { return Err("云文档选择无效，请重新选择。".into()); }
        if !ids.contains(id) { ids.push(id.clone()); }
    }
    if ids.len()>2 { return Err("一次最多附两篇已保存的云文档。".into()); }
    Ok(ids)
}
fn preview_documents(payload: &Value) -> Result<Vec<Value>, String> {
    let documents=match payload.get("documents") {
        Some(value)=>value.as_array().cloned().ok_or("云文档预览数据损坏，请重新预览。")?,
        None=>payload.get("document").filter(|d|!d.is_null()).cloned().into_iter().collect(),
    };
    if documents.len()>2 || documents.iter().any(|d|d["id"].as_str().filter(|s|!s.is_empty()).is_none() || d["documentUrl"].as_str().filter(|s|!s.is_empty()).is_none() || d["revision"].as_i64().is_none()) {
        return Err("云文档预览数据不完整，请重新预览。".into());
    }
    Ok(documents)
}
fn check_document_snapshot(current: &Value, preview: &Value) -> Result<(), String> {
    if ["id","revision","documentUrl"].iter().any(|key|current[*key]!=preview[*key]) {
        return Err("云文档版本或链接已变化，请重新预览发送。".into());
    }
    Ok(())
}
fn display_names(value: &str) -> String {
    let names = meeting_settings::names(value);
    if names.is_empty() { "未填写".into() } else { names.join("、") }
}
fn parse_review_time(value: &str) -> Result<chrono::NaiveDateTime, chrono::ParseError> {
    chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M"))
}
fn display_time(value: &str) -> String {
    parse_review_time(value).map(|time| time.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|_| value.to_owned())
}
fn card(review: &Review) -> Value {
    json!({"config":{"wide_screen_mode":true},"header":{"template":"blue","title":{"tag":"plain_text","content":format!("会议纪要｜{}",review.title.trim())}},
        "elements":[
            {"tag":"div","text":{"tag":"plain_text","content":format!("会议时间（北京时间）\n{}",display_time(&review.meeting_time))}},
            {"tag":"div","text":{"tag":"plain_text","content":format!("出席人员：{}",display_names(&review.participants))}},
            {"tag":"div","text":{"tag":"plain_text","content":format!("缺席人员：{}",display_names(&review.absent))}},
            {"tag":"div","text":{"tag":"plain_text","content":format!("主持人：{}",display_names(&review.host))}},
            {"tag":"hr"}, {"tag":"div","text":{"tag":"plain_text","content":review.summary}},
            {"tag":"note","elements":[{"tag":"plain_text","content":"由 Meetily 本地录音生成，已人工审核"}]}
        ]})
}
fn review_card(review: &Review, documents: &[Value]) -> Result<Value,String> {
    let mut value=card(review);
    if let Some(pattern)=&review.message_template {
        let time=display_time(&review.meeting_time);let present=display_names(&review.participants);let absent=display_names(&review.absent);let host=display_names(&review.host);
        let links=if documents.is_empty(){"未附云文档".into()}else{documents.iter().map(|d|d["documentUrl"].as_str().unwrap_or("")).collect::<Vec<_>>().join("\n")};
        let body=super::message_templates::render(pattern,&[("会议名称",&review.title),("会议时间",&time),("出席人员",&present),("缺席人员",&absent),("主持人",&host),("概要",&review.summary),("云文档链接",&links)])?;
        value["elements"]=json!([{"tag":"div","text":{"tag":"plain_text","content":body}}]);
    }
    for document in documents {
        let link=document["documentUrl"].as_str().filter(|s|!s.is_empty()).ok_or("云文档链接缺失，请先保存云文档。")?;
        let label=if link.contains("/wiki/"){"查看知识库纪要"}else{"查看云空间纪要"};
        value["elements"].as_array_mut().ok_or("卡片格式无效")?.push(json!({"tag":"action","actions":[{"tag":"button","text":{"tag":"plain_text","content":label},"url":link,"type":"primary"}]}));
    }
    Ok(value)
}
async fn source(pool: &SqlitePool, id: &str) -> Result<(Value, String, String, String), String> {
    let row = sqlx::query("SELECT title,created_at FROM meetings WHERE id=?").bind(id).fetch_optional(pool).await.map_err(db_error)?.ok_or("当前公司不存在该会议。")?;
    let file = markdown_export::preview(pool,id).await?.ok_or("请先生成并保存会议纪要。")?;
    let settings = meeting_settings::read(pool,id).await?;
    let title: String = row.get("title");
    let created: String = row.get("created_at");
    let revision = fingerprint(&[&title,&created,&file.to_string(),&serde_json::to_string(&settings).map_err(|_|"人员信息无效")?]);
    Ok((file,title,created,revision))
}

#[tauri::command]
pub async fn meeting_processing_stats(state: tauri::State<'_,AppState>, meeting_id: String) -> Result<Value,String> {
    let row=sqlx::query("SELECT t.transcription_seconds,t.audio_seconds,t.model,t.provider,s.processing_time FROM meetings m LEFT JOIN meeting_processing_timings t ON t.meeting_id=m.id LEFT JOIN summary_processes s ON s.meeting_id=m.id AND LOWER(s.status)='completed' WHERE m.id=?")
        .bind(meeting_id).fetch_optional(state.db_manager.pool()).await.map_err(db_error)?.ok_or("会议不存在。")?;
    Ok(json!({"transcriptionSeconds":row.get::<Option<f64>,_>("transcription_seconds"),"audioSeconds":row.get::<Option<f64>,_>("audio_seconds"),"model":row.get::<Option<String>,_>("model"),"provider":row.get::<Option<String>,_>("provider"),"summarySeconds":row.get::<Option<f64>,_>("processing_time")}))
}

#[tauri::command]
pub async fn lark_manual_context(state: tauri::State<'_,AppState>, meeting_id: String) -> Result<Value,String> {
    let _storage=crate::audio::storage_migration::use_storage().await?;
    let pool=state.db_manager.pool();
    crate::summary::attendance::refresh(pool,&meeting_id).await?;
    let (file,title,created,_) = source(pool,&meeting_id).await?;
    let settings=meeting_settings::read(pool,&meeting_id).await?;
    let c=app::load(pool).await?;
    let default_chat_id=associated_chat(pool,&meeting_id,&c).await?;
    let documents=sqlx::query("SELECT id,title,project,document_url,saved_hash,status FROM lark_editable_documents WHERE app_id=? AND meeting_id=? AND (credential_owner='' OR credential_owner=?) AND document_url IS NOT NULL ORDER BY updated_at DESC")
        .bind(&c.app_id).bind(&meeting_id).bind(&c.owner_id).fetch_all(pool).await.map_err(db_error)?;
    let timestamp=chrono::DateTime::parse_from_rfc3339(&created).map(|d|d.timestamp()).ok()
        .or_else(||chrono::NaiveDateTime::parse_from_str(&created,"%Y-%m-%d %H:%M:%S%.f").ok().map(|d|d.and_utc().timestamp()));
    let time=timestamp.and_then(|t|chrono::DateTime::from_timestamp(t,0)).map(|d|d.with_timezone(&chrono::FixedOffset::east_opt(8*3600).unwrap()).format("%Y-%m-%dT%H:%M:%S").to_string()).unwrap_or_default();
    let recent=sqlx::query("SELECT status,detail,sent_at,payload FROM lark_manual_drafts WHERE meeting_id=? AND status!='preview' ORDER BY created_at DESC,rowid DESC LIMIT 1").bind(&meeting_id).fetch_optional(pool).await.map_err(db_error)?;
    Ok(json!({"defaultChatId":default_chat_id,"title":title,"meetingTime":time,"participants":settings.participants,"absent":settings.absent,"host":settings.host,"attendance":settings.attendance,
        "summary":super::card::summary_overview(file["content"].as_str().unwrap_or("")),"markdown":file,
        "documents":documents.iter().map(|r|json!({"id":r.get::<String,_>("id"),"title":r.get::<Option<String>,_>("title").unwrap_or_else(||title.clone()),"project":r.get::<String,_>("project"),"documentUrl":r.get::<String,_>("document_url"),"available":r.get::<String,_>("status")=="ready"&&r.get::<Option<String>,_>("saved_hash").as_deref()==file["hash"].as_str()})).collect::<Vec<_>>(),
        "lastDelivery":recent.map(|r|json!({"status":r.get::<String,_>("status"),"detail":r.get::<String,_>("detail"),"sentAt":r.get::<Option<i64>,_>("sent_at")}))}))
}

// Match the actual recorded meeting instance, never a reusable meeting number or
// another meeting's last-used chat. Conflicting destinations require manual choice.
async fn associated_chat(pool: &SqlitePool, meeting_id: &str, c: &AppConfig) -> Result<Option<String>,String> {
    if c.app_id.is_empty() || c.owner_id.is_empty() { return Ok(None); }
    let chats: Vec<String> = sqlx::query_scalar(
        "WITH linked AS (
            SELECT l.remote_id FROM meetings m JOIN lark_recording_links l ON l.folder_path=m.folder_path
            WHERE m.id=?1 AND l.app_id=?2 AND l.owner_id=?3 AND l.blocked=0 AND l.remote_id IS NOT NULL
        )
        SELECT chat_id FROM lark_app_routes WHERE app_id=?2 AND owner_id=?3 AND remote_id IN (SELECT remote_id FROM linked)
        UNION SELECT chat_id FROM lark_app_deliveries WHERE meeting_id=?1 AND app_id=?2 AND remote_id IN (SELECT remote_id FROM linked)
        UNION SELECT target_chat_id FROM recording_jobs WHERE meeting_id=?1 AND target_chat_id IS NOT NULL AND EXISTS(SELECT 1 FROM linked)"
    ).bind(meeting_id).bind(&c.app_id).bind(&c.owner_id).fetch_all(pool).await.map_err(db_error)?;
    Ok(if chats.len()==1 && valid_chat(&chats[0]) { chats.into_iter().next() } else { None })
}

#[tauri::command]
pub async fn lark_manual_groups(state: tauri::State<'_,AppState>) -> Result<Vec<Value>,String> {
    let c=configured(state.db_manager.pool()).await?;
    let token=app::tenant_token(&c).await?;
    let mut groups=Vec::new();
    let mut cursor=String::new();
    for _ in 0..50 {
        let path={
            let mut query=url::form_urlencoded::Serializer::new(String::new());
            query.append_pair("page_size","100");
            if !cursor.is_empty() { query.append_pair("page_token",&cursor); }
            format!("/open-apis/im/v1/chats?{}",query.finish())
        };
        let data=app::api(reqwest::Method::GET,&path,&token,None).await?;
        if let Some(items)=data["items"].as_array() {
            for item in items {
                if let Some(id)=item["chat_id"].as_str().filter(|id|valid_chat(id)) {
                    groups.push(json!({"id":id,"name":item["name"].as_str().unwrap_or("未命名群")}));
                }
            }
        }
        if data["has_more"].as_bool()!=Some(true) { return Ok(groups); }
        let next=data["page_token"].as_str().filter(|s| !s.is_empty() && *s!=cursor).ok_or("群列表分页异常，请重试。")?;
        cursor=next.into();
    }
    Err("群数量超过当前加载上限，请联系管理员缩小机器人使用范围。".into())
}

#[tauri::command]
pub async fn lark_manual_preview(state: tauri::State<'_,AppState>, meeting_id: String, review: Review) -> Result<Value,String> {
    let _storage=crate::audio::storage_migration::use_storage().await?;
    let _config=app::CONFIG_LOCK.lock().await;
    let _guard=app::DELIVERY_LOCK.lock().await;
    validate(&review)?;
    let pool=state.db_manager.pool();
    let c=configured(pool).await?;
    let mut settings=crate::summary::attendance::refresh(pool,&meeting_id).await?;
    if !review.participants_edited&&review.participants!=settings.participants||!review.absent_edited&&review.absent!=settings.absent{return Err("本场人员名单已更新，请重新打开发送窗口核对。".into());}
    if review.participants_edited||review.absent_edited{
        let metadata=settings.attendance.get_or_insert_with(Default::default);
        if review.participants_edited{settings.participants=review.participants.clone();metadata.participants_manual=true;}
        if review.absent_edited{settings.absent=review.absent.clone();metadata.absent_manual=true;}
        meeting_settings::save(pool,&meeting_id,&settings).await?;
    }
    let (file,_,_,revision)=source(pool,&meeting_id).await?;
    let token=app::tenant_token(&c).await?;
    let chat=app::api(reqwest::Method::GET,&format!("/open-apis/im/v1/chats/{}",review.chat_id),&token,None).await?;
    let name=chat["name"].as_str().filter(|s|!s.is_empty()).ok_or("无法核对目标群名称，未准备发送。")?;
    if !review.project.is_empty(){return Err("云文档已改为独立保存，请先在云文档窗口保存，再点击发送到群。".into());}
    let mut documents=Vec::new();
    for id in selected_document_ids(&review)? {
        documents.push(super::cloud_editor::for_send(pool,&c,&token,&meeting_id,&id,file["hash"].as_str().ok_or("文件校验值缺失")?).await?);
    }
    let document=documents.first().cloned().unwrap_or(Value::Null);
    let value=review_card(&review,&documents)?;
    let archive=Value::Null;
    if value.to_string().len()>19*1024 { return Err("纪要卡片过长，请精简人员或概要。".into()); }
    let id=uuid::Uuid::new_v4().simple().to_string();
    let original=file["content"].as_str().ok_or("完整纪要缺失")?;
    let body=original.lines().enumerate().filter(|(i,line)|!(*i==0&&line.starts_with("# "))&&!line.starts_with("会议时间：")&&!line.starts_with("本地会议 ID：")).map(|(_,line)|line).collect::<Vec<_>>().join("\n");
    let document_content=format!("# {}\n会议时间（北京时间）：{}\n出席人员：{}\n缺席人员：{}\n主持人：{}\n\n{}",review.title,display_time(&review.meeting_time),display_names(&review.participants),display_names(&review.absent),display_names(&review.host),body);
    let payload=json!({"chatId":review.chat_id,"chatName":name,"card":value,"markdown":file,"review":review,"archive":archive,"documentContent":document_content,"document":document,"documents":documents});
    sqlx::query("INSERT INTO lark_manual_drafts(id,meeting_id,app_id,config_revision,source_revision,payload,created_at) VALUES(?,?,?,?,?,?,?)")
        .bind(&id).bind(&meeting_id).bind(&c.app_id).bind(config_revision(&c)).bind(revision).bind(payload.to_string()).bind(now()).execute(pool).await.map_err(db_error)?;
    Ok(json!({"id":id,"chatName":name,"card":value,"markdown":file,"includeMarkdown":review.include_markdown,"archive":archive,"document":document,"documents":documents}))
}

#[tauri::command]
pub async fn lark_manual_send(state: tauri::State<'_,AppState>, draft_id: String, acknowledge_duplicate_risk: bool) -> Result<Value,String> {
    let _storage=crate::audio::storage_migration::use_storage().await?;
    let _config=app::CONFIG_LOCK.lock().await;
    let _guard=app::DELIVERY_LOCK.lock().await;
    let pool=state.db_manager.pool();
    let c=configured(pool).await?;
    let row=sqlx::query("SELECT * FROM lark_manual_drafts WHERE id=? AND app_id=?").bind(&draft_id).bind(&c.app_id).fetch_optional(pool).await.map_err(db_error)?.ok_or("发送预览已失效，请重新预览。")?;
    let status:String=row.get("status");
    if status=="sent" { return Ok(json!({"sentAt":row.get::<Option<i64>,_>("sent_at"),"alreadySent":true})); }
    if status!="preview" { return Err("上次发送结果不确定，请先检查群消息；如需再次发送，请重新预览并确认。".into()); }
    if row.get::<String,_>("config_revision")!=config_revision(&c) { return Err("会议助手配置已变化，请重新预览。".into()); }
    let meeting:String=row.get("meeting_id");
    let prior:Option<(String,String,Option<i64>)>=sqlx::query_as("SELECT status,payload,sent_at FROM lark_manual_drafts WHERE meeting_id=? AND app_id=? AND id!=? AND status!='preview' ORDER BY created_at DESC,rowid DESC LIMIT 1")
        .bind(&meeting).bind(&c.app_id).bind(&draft_id).fetch_optional(pool).await.map_err(db_error)?;
    if let Some((prior_status,prior_payload,sent_at))=prior {
        if prior_status=="sent" && prior_payload==row.get::<String,_>("payload") {
            return Ok(json!({"sentAt":sent_at,"alreadySent":true}));
        }
        if !acknowledge_duplicate_risk {return Err("本场会议已有发送记录，请核对群消息并确认再次发送。".into());}
    }
    crate::summary::attendance::refresh(pool,&meeting).await?;
    let (file,_,_,revision)=source(pool,&meeting).await?;
    if revision!=row.get::<String,_>("source_revision") { return Err("纪要或人员信息已变化，请重新预览。".into()); }
    let payload:Value=serde_json::from_str(&row.get::<String,_>("payload")).map_err(|_|"预览数据损坏。")?;
    if payload["review"]["attendanceConfirmed"].as_bool()!=Some(true){return Err("请重新预览并确认出席、缺席名单后发送。".into());}
    let token=app::tenant_token(&c).await?;
    let chat=payload["chatId"].as_str().filter(|s|valid_chat(s)).ok_or("目标群无效。")?.to_owned();
    let current=app::api(reqwest::Method::GET,&format!("/open-apis/im/v1/chats/{chat}"),&token,None).await?;
    if current["name"]!=payload["chatName"] { return Err("目标群名称已变化，请重新预览。".into()); }
    if !payload["archive"].is_null(){return Err("旧版发送预览已失效，请先独立保存云文档，再重新预览发送。".into());}
    let mut document_urls=Vec::new();
    for document in preview_documents(&payload)? {
        let id=document["id"].as_str().ok_or("云文档标识缺失")?;
        let current=super::cloud_editor::for_send(pool,&c,&token,&meeting,id,file["hash"].as_str().ok_or("文件校验值缺失")?).await?;
        check_document_snapshot(&current,&document)?;
        document_urls.push(current["documentUrl"].as_str().ok_or("云文档链接缺失")?.to_owned());
    }
    let include_markdown=payload["review"]["includeMarkdown"].as_bool().unwrap_or(false);
    // Upload only after confirmation; failure here must not send even the card.
    let file_key=if include_markdown{Some(app::upload_markdown(&token,&payload["markdown"]).await?)}else{None};
    send_reviewed_messages(pool,&draft_id,&chat,&payload["card"],file_key,document_urls,|body|{let token=&token;async move{app::api(reqwest::Method::POST,"/open-apis/im/v1/messages?receive_id_type=chat_id",token,Some(body)).await}}).await
}

async fn send_reviewed_messages<F,Fut>(pool:&SqlitePool,draft_id:&str,chat:&str,card:&Value,file_key:Option<String>,document_urls:Vec<String>,mut send:F)->Result<Value,String>
where F:FnMut(Value)->Fut,Fut:std::future::Future<Output=Result<Value,String>> {
    sqlx::query("UPDATE lark_manual_drafts SET status='sending',attachment_status=?,detail='发送中，若中断请先核对群消息' WHERE id=? AND status='preview'").bind(if file_key.is_some(){"pending"}else{"not_selected"}).bind(&draft_id).execute(pool).await.map_err(db_error)?;
    let result=send(json!({"receive_id":chat,"msg_type":"interactive","content":card.to_string(),"uuid":draft_id})).await;
    match result {
        Ok(data) if data["message_id"].as_str().is_some() => {
            let sent=now();
            if let Some(key)=file_key {
                sqlx::query("UPDATE lark_manual_drafts SET message_id=?,sent_at=?,attachment_status='sending',detail='纪要卡片已发送，正在发送 Markdown 附件' WHERE id=?")
                    .bind(data["message_id"].as_str()).bind(sent).bind(&draft_id).execute(pool).await.map_err(|_|"卡片已发送，本地记录失败，尚未发送附件，请核对群消息。")?;
                let response=send(attachment_message(chat,draft_id,&key)).await;
                match response {
                    Ok(file_data) if file_data["message_id"].as_str().is_some()=>{
                        sqlx::query("UPDATE lark_manual_drafts SET attachment_status='sent',attachment_message_id=? WHERE id=?").bind(file_data["message_id"].as_str()).bind(&draft_id).execute(pool).await.map_err(|_|"卡片和附件已发送，但本地记录失败，请核对群消息。")?;
                    },
                    result=>{let detail=format!("纪要卡片已发送，Markdown 附件未确认成功：{} 请检查群消息，不会自动重试。",result.err().unwrap_or("未返回文件消息标识".into()));sqlx::query("UPDATE lark_manual_drafts SET status='partial',attachment_status='unknown',detail=? WHERE id=?").bind(&detail).bind(&draft_id).execute(pool).await.map_err(db_error)?;return Err(detail);}
                }
            }
            sqlx::query("UPDATE lark_manual_drafts SET status='sent',detail='纪要卡片已发送',sent_at=?,message_id=? WHERE id=?")
                .bind(sent).bind(data["message_id"].as_str()).bind(&draft_id).execute(pool).await.map_err(|_|"消息已发送，但本地记录保存失败，请先核对群消息。")?;
            Ok(json!({"sentAt":sent,"alreadySent":false,"documentUrl":document_urls.first(),"documentUrls":document_urls}))
        }
        result => {
            let detail=result.err().unwrap_or("未返回消息标识，发送结果不确定。".into());
            sqlx::query("UPDATE lark_manual_drafts SET status='unknown',detail=? WHERE id=?").bind(&detail).bind(&draft_id).execute(pool).await.map_err(db_error)?;
            Err(format!("{detail} 请核对群消息后再决定是否重新预览发送；不会自动重试。"))
        }
    }
}

fn attachment_message(chat:&str,draft:&str,key:&str)->Value{
    let uuid=fingerprint(&[draft,"markdown"]);
    json!({"receive_id":chat,"msg_type":"file","content":json!({"file_key":key}).to_string(),"uuid":&uuid[..32]})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn default_chat_requires_unique_recorded_instance_and_current_identity() {
        let pool=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE meetings(id TEXT PRIMARY KEY,folder_path TEXT); INSERT INTO meetings VALUES('local','folder'); INSERT INTO meetings VALUES('unlinked','other');")
            .execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261015000000_add_lark_app.sql")).execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261015040000_add_recording_jobs.sql")).execute(&pool).await.unwrap();
        let mut c=AppConfig{app_id:"cli_test".into(),owner_id:"ou_test".into(),..Default::default()};
        assert_eq!(associated_chat(&pool,"local",&c).await.unwrap(),None);
        sqlx::raw_sql("INSERT INTO lark_recording_links(folder_path,app_id,owner_id,remote_id,meeting_no,started_at) VALUES('folder','cli_test','ou_test','instance','123456789',1);
            INSERT INTO lark_app_routes VALUES('cli_test','ou_test','123456789','number','oc_bound','instance',0);
            INSERT INTO lark_app_routes VALUES('cli_test','ou_other','123456789','number','oc_other','instance',9999999999);
            INSERT INTO lark_app_routes VALUES('cli_other','ou_test','123456789','number','oc_other','instance',9999999999);
            INSERT INTO lark_app_routes VALUES('cli_test','ou_test','123456789','number','oc_reused','another_instance',9999999999);")
            .execute(&pool).await.unwrap();
        // The actual instance remains associated after the six-hour recording window.
        assert_eq!(associated_chat(&pool,"local",&c).await.unwrap().as_deref(),Some("oc_bound"));
        assert_eq!(associated_chat(&pool,"unlinked",&c).await.unwrap(),None);
        c.owner_id="ou_other".into();assert_eq!(associated_chat(&pool,"local",&c).await.unwrap(),None);
        c.owner_id="ou_test".into();c.app_id="cli_other".into();assert_eq!(associated_chat(&pool,"local",&c).await.unwrap(),None);
        c.app_id="cli_test".into();
        sqlx::query("INSERT INTO lark_app_routes VALUES('cli_test','ou_test','instance','id','oc_second','instance',0)").execute(&pool).await.unwrap();
        assert_eq!(associated_chat(&pool,"local",&c).await.unwrap(),None);
        sqlx::query("DELETE FROM lark_app_routes WHERE chat_id='oc_second'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE lark_recording_links SET blocked=1").execute(&pool).await.unwrap();
        assert_eq!(associated_chat(&pool,"local",&c).await.unwrap(),None);
        sqlx::query("UPDATE lark_recording_links SET blocked=0").execute(&pool).await.unwrap();
        sqlx::query("DELETE FROM lark_app_routes").execute(&pool).await.unwrap();
        // A previously frozen delivery can recover the association, but not override a conflict.
        sqlx::query("INSERT INTO lark_app_deliveries VALUES('local','cli_test','instance','oc_saved','uuid','text','sent','','msg',1)").execute(&pool).await.unwrap();
        assert_eq!(associated_chat(&pool,"local",&c).await.unwrap().as_deref(),Some("oc_saved"));
        sqlx::query("INSERT INTO recording_jobs(meeting_id,folder_path,status,options_json,target_chat_id,created_at,updated_at) VALUES('local','folder','completed','{}','oc_conflict',1,1)").execute(&pool).await.unwrap();
        assert_eq!(associated_chat(&pool,"local",&c).await.unwrap(),None);
    }
    #[tokio::test]async fn card_and_file_have_separate_receipts_and_partial_failure_stops(){
        for (attach,fail_card,fail_file,expected_status,count) in [(false,false,false,"sent",1),(true,false,false,"sent",2),(true,false,true,"partial",2),(true,true,false,"unknown",1)]{
            let pool=SqlitePool::connect("sqlite::memory:").await.unwrap();
            sqlx::raw_sql("CREATE TABLE lark_manual_drafts(id TEXT,status TEXT,detail TEXT,sent_at INTEGER,message_id TEXT,attachment_status TEXT,attachment_message_id TEXT);INSERT INTO lark_manual_drafts(id,status) VALUES('draft','preview');").execute(&pool).await.unwrap();
            let mut requests=Vec::new();let outcome=send_reviewed_messages(&pool,"draft","oc_test",&json!({"elements":[]}),attach.then(||"file_key".into()),vec!["https://team.larksuite.com/docx/a".into(),"https://team.larksuite.com/wiki/b".into()],|body|{
                requests.push(body.clone());let failure=if body["msg_type"]=="file"{fail_file}else{fail_card};async move{if failure{Err("模拟网络结果不确定".into())}else{Ok(json!({"message_id":if body["msg_type"]=="file"{"file-message"}else{"card-message"}}))}}
            }).await;
            assert_eq!(requests.len(),count);assert_eq!(outcome.is_ok(),expected_status=="sent");
            if let Ok(value)=outcome {assert_eq!(value["documentUrls"].as_array().unwrap().len(),2);}
            let row=sqlx::query("SELECT * FROM lark_manual_drafts").fetch_one(&pool).await.unwrap();assert_eq!(row.get::<String,_>("status"),expected_status);
            if attach&&!fail_card {assert_eq!(row.get::<String,_>("message_id"),"card-message");assert_ne!(requests[0]["uuid"],requests[1]["uuid"]);}
            if attach&&!fail_card&&!fail_file{assert_eq!(row.get::<String,_>("attachment_message_id"),"file-message");assert_eq!(row.get::<String,_>("attachment_status"),"sent");}
        }
    }
    #[test]fn attachment_opt_in_defaults_off_and_uses_a_distinct_stable_message_id(){
        let mut value=serde_json::to_value(review()).unwrap();value.as_object_mut().unwrap().remove("includeMarkdown");
        value.as_object_mut().unwrap().remove("documentIds");
        let r:Review=serde_json::from_value(value).unwrap();assert!(!r.include_markdown);assert!(r.document_id.is_none());assert!(r.document_ids.is_empty());
        let a=attachment_message("oc_test","draft","file_1");let b=attachment_message("oc_test","draft","file_1");assert_eq!(a,b);assert_ne!(a["uuid"],"draft");assert_eq!(a["msg_type"],"file");assert_eq!(serde_json::from_str::<Value>(a["content"].as_str().unwrap()).unwrap()["file_key"],"file_1");
    }
    fn review() -> Review { Review {chat_id:"oc_test".into(),title:"战略会".into(),meeting_time:"2026-11-14T09:30".into(),participants:"甲、乙".into(),absent:"丙、丁".into(),host:"甲".into(),summary:"确认项目安排".into(),project:String::new(),document_id:None,document_ids:Vec::new(),message_template:None,include_markdown:false,attendance_confirmed:true,participants_edited:false,absent_edited:false} }
    fn documents() -> Vec<Value> {vec![json!({"id":"drive","documentUrl":"https://team.larksuite.com/docx/a","revision":4}),json!({"id":"wiki","documentUrl":"https://team.larksuite.com/wiki/b","revision":8})]}
    #[test]fn multiple_links_are_explicit_deduplicated_and_legacy_selection_is_supported(){
        let mut r=review();assert!(selected_document_ids(&r).unwrap().is_empty());
        r.document_id=Some("drive".into());assert_eq!(selected_document_ids(&r).unwrap(),vec!["drive"]);
        r.document_ids=vec!["drive".into(),"wiki".into(),"wiki".into()];assert_eq!(selected_document_ids(&r).unwrap(),vec!["drive","wiki"]);
        r.document_ids.push("third".into());assert!(validate(&r).is_err());
        r.document_ids=vec!["".into()];assert!(validate(&r).is_err());
    }
    #[test]fn both_saved_links_appear_in_standard_and_custom_cards(){
        let mut r=review();let docs=documents();
        for pattern in [None,Some("纪要：{云文档链接}".into())]{
            r.message_template=pattern;
            let value=review_card(&r,&docs).unwrap();let elements=value["elements"].as_array().unwrap();
            let actions=elements.iter().filter(|v|v["tag"]=="action").collect::<Vec<_>>();assert_eq!(actions.len(),2);
            assert_eq!(actions[0]["actions"][0]["url"],docs[0]["documentUrl"]);assert_eq!(actions[1]["actions"][0]["url"],docs[1]["documentUrl"]);
            assert_eq!(actions[0]["actions"][0]["text"]["content"],"查看云空间纪要");assert_eq!(actions[1]["actions"][0]["text"]["content"],"查看知识库纪要");
            if r.message_template.is_some(){let body=elements[0]["text"]["content"].as_str().unwrap();assert!(body.contains("/docx/a\nhttps://team.larksuite.com/wiki/b"));}
        }
        assert!(!review_card(&review(),&[]).unwrap()["elements"].as_array().unwrap().iter().any(|v|v["tag"]=="action"));
    }
    #[test]fn old_drafts_remain_readable_and_unsaved_or_changed_documents_are_rejected(){
        let docs=documents();assert_eq!(preview_documents(&json!({"documents":docs})).unwrap(),docs);
        assert_eq!(preview_documents(&json!({"document":docs[0]})).unwrap(),vec![docs[0].clone()]);
        assert!(preview_documents(&json!({"document":null})).unwrap().is_empty());
        assert!(preview_documents(&json!({"documents":[{"id":"not-saved","documentUrl":null,"revision":null}]})).is_err());
        assert!(preview_documents(&json!({"documents":null})).is_err());
        for key in ["revision","documentUrl"] {let mut changed=docs[1].clone();changed[key]=json!("changed");assert!(check_document_snapshot(&changed,&docs[1]).is_err());}
        assert!(check_document_snapshot(&docs[0],&docs[0]).is_ok());
    }
    #[test]
    fn metadata_uses_seconds_and_inline_names() {
        let mut r=review(); r.participants="甲\n乙，甲".into(); r.absent="丙；丁".into(); validate(&r).unwrap(); let c=card(&r);
        assert_eq!(c["elements"][0]["text"]["content"],"会议时间（北京时间）\n2026-11-14 09:30:00");
        assert_eq!(c["elements"][1]["text"]["content"],"出席人员：甲、乙");
        assert_eq!(c["elements"][2]["text"]["content"],"缺席人员：丙、丁");
        r.meeting_time="2026-11-14T09:30:42".into(); validate(&r).unwrap();
        assert_eq!(card(&r)["elements"][0]["text"]["content"],"会议时间（北京时间）\n2026-11-14 09:30:42");
    }
    #[test]
    fn invalid_destination_date_and_conflicting_attendance_are_rejected() {
        let mut unconfirmed=review();unconfirmed.attendance_confirmed=false;assert!(validate(&unconfirmed).unwrap_err().contains("确认名单"));
        let mut r=review();r.chat_id="oc_bad?url=evil".into();assert!(validate(&r).is_err());
        r=review();r.meeting_time="2026-02-30T09:30".into();assert!(validate(&r).is_err());
        r=review();r.absent="甲".into();assert!(validate(&r).is_err());
    }
}
