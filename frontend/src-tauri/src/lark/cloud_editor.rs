use super::{app::{self, db_error, now, AppConfig}, documents,locations};
use crate::{state::AppState, summary::markdown_export};
use reqwest::Method;
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};
use std::future::Future;

async fn replace_content<F, Fut>(doc:&str, revision:i64, old_count:usize, content:Value, mut request:F)->Result<i64,String>
where F:FnMut(Method,String,Value)->Fut, Fut:Future<Output=Result<Value,String>> {
    let insertion=request(Method::POST,format!("/open-apis/docx/v1/documents/{doc}/blocks/{doc}/descendant?document_revision_id={revision}&client_token={}",uuid::Uuid::new_v4()),content).await.map_err(|e|format!("写入新正文失败：{e}"))?;
    let mut revision=insertion["document_revision_id"].as_i64().ok_or("新内容可能已写入，但未返回版本；请打开 Lark 核对。")?;
    if old_count>0 {
        let deletion=request(Method::DELETE,format!("/open-apis/docx/v1/documents/{doc}/blocks/{doc}/children/batch_delete?document_revision_id={revision}&client_token={}",uuid::Uuid::new_v4()),json!({"start_index":0,"end_index":old_count})).await?;
        revision=deletion["document_revision_id"].as_i64().ok_or("云文档可能已保存，但未返回版本；请打开 Lark 核对。")?;
    }
    Ok(revision)
}

async fn configured(pool: &SqlitePool) -> Result<AppConfig, String> {
    let c = app::load(pool).await?;
    if !c.enabled || c.app_id.is_empty() || c.secret.is_empty() {
        return Err("请先配置并启用会议助手。".into());
    }
    Ok(c)
}
fn view(row: &SqliteRow) -> Value {
    json!({"id":row.get::<String,_>("id"),"project":row.get::<String,_>("project"),
        "documentUrl":row.get::<Option<String>,_>("document_url"),"revision":row.get::<Option<i64>,_>("revision"),
        "savedHash":row.get::<Option<String>,_>("saved_hash"),"status":row.get::<String,_>("status"),
        "detail":row.get::<String,_>("detail"),"title":row.get::<Option<String>,_>("title")})
}
async fn remote_revision(token: &str, doc: &str) -> Result<i64, String> {
    let data = app::api(Method::GET, &format!("/open-apis/docx/v1/documents/{doc}"), token, None).await?;
    data["document"]["revision_id"].as_i64().filter(|v| *v >= 0).ok_or("未能读取云文档版本，已停止写入。".into())
}
fn check_version(status: &str, saved: Option<i64>, expected: Option<i64>, remote: i64) -> Result<(), String> {
    if status != "ready" { return Err("上次云文档保存结果尚未确认，请打开 Lark 核对，当前不会继续覆盖或发送。".into()); }
    if saved != expected { return Err("此窗口中的云文档版本已过期，请关闭后重新打开。".into()); }
    if saved != Some(remote) { return Err("云文档已被其他操作修改。为保留同事的更改，本次未覆盖；本地草稿仍保留，请打开 Lark 核对。".into()); }
    Ok(())
}

#[tauri::command]
pub async fn lark_document_status(state: tauri::State<'_, AppState>, meeting_id: String, project: String) -> Result<Value, String> {
    let pool=state.db_manager.pool(); let c=app::load(pool).await?;
    let row=sqlx::query("SELECT * FROM lark_editable_documents WHERE app_id=? AND meeting_id=? AND project=? AND (credential_owner='' OR credential_owner=?)")
        .bind(c.app_id).bind(meeting_id).bind(project).bind(c.owner_id).fetch_optional(pool).await.map_err(db_error)?;
    Ok(row.as_ref().map(view).unwrap_or(Value::Null))
}

// This command writes only the document. It never loads groups or calls message APIs.
#[tauri::command]
pub async fn lark_document_save(state: tauri::State<'_, AppState>, meeting_id: String, project: String,
    expected_hash: String, expected_revision: Option<i64>, title:Option<String>) -> Result<Value, String> {
    let _storage=crate::audio::storage_migration::use_storage().await?;
    let _config=app::CONFIG_LOCK.lock().await;
    let _guard=app::DELIVERY_LOCK.lock().await;
    let pool=state.db_manager.pool(); let c=configured(pool).await?;
    let file=markdown_export::preview(pool,&meeting_id).await?.ok_or("请先保存本地纪要草稿。")?;
    if file["hash"].as_str()!=Some(&expected_hash) {return Err("本地文件已变化，请重新打开云文档编辑器。".into());}
    let markdown=file["content"].as_str().filter(|s|!s.trim().is_empty()).ok_or("纪要内容为空。")?;
    let title=validate_title(title.as_deref().unwrap_or_else(||markdown.lines().find_map(|s|s.strip_prefix("# ")).unwrap_or("会议纪要")))?;
    let (target,parent)=locations::load(pool,&c,&project).await?;
    let existing=sqlx::query("SELECT * FROM lark_editable_documents WHERE app_id=? AND meeting_id=? AND project=?")
        .bind(&c.app_id).bind(&meeting_id).bind(&project).fetch_optional(pool).await.map_err(db_error)?;
    let token=locations::access_token(pool,&c,&target.owner).await?;
    let mut previous_children=0;
    let mut previous_revision=None;
    if let Some(row)=&existing {
        if row.get::<String,_>("parent_url")!=parent {return Err("该项目目录已改变。已有文档不会自动移动，请使用原目录或另建项目。".into());}
        let doc:Option<String>=row.get("document_id");
        let doc=doc.ok_or("上次创建结果未确认，请先核对知识库，避免重复创建。")?;
        let revision=remote_revision(&token,&doc).await?;
        check_version(&row.get::<String,_>("status"),row.get("revision"),expected_revision,revision)?;
        if row.get::<String,_>("credential_owner")!=target.owner{return Err("文档所属账号已改变，请重新选择位置。".into());}
        if row.get::<Option<String>,_>("saved_hash").as_deref()==Some(&expected_hash){let saved=save_title(pool,&token,&target,row,&title).await?;locations::remember(pool,&c,&project).await?;return Ok(saved);}
        let root=app::api(Method::GET,&format!("/open-apis/docx/v1/documents/{doc}/blocks/{doc}?document_revision_id={revision}"),&token,None).await?;
        previous_children=match root["block"].get("children") {
            Some(Value::Array(children))=>children.len(),
            None|Some(Value::Null)=>0,
            _=>return Err("云文档目录结构异常，未修改文档。".into())
        };
        previous_revision=Some(revision);
    } else if expected_revision.is_some() {return Err("云文档关联已变化，请重新打开编辑器。".into());}
    let converted=app::api(Method::POST,"/open-apis/docx/v1/documents/blocks/convert",&token,
        Some(json!({"content_type":"markdown","content":markdown}))).await
        .map_err(|e|format!("未修改云文档。请核对文本转换权限 docx:document.block:convert。{e}"))?;
    let content=documents::nested_blocks(&converted)?;
    let id=existing.as_ref().map(|r|r.get::<String,_>("id")).unwrap_or_else(||uuid::Uuid::new_v4().to_string());
    if existing.is_none() {
        sqlx::query("INSERT INTO lark_editable_documents(id,app_id,meeting_id,project,parent_url,status,updated_at,credential_owner) VALUES(?,?,?,?,?,'creating',?,?)")
            .bind(&id).bind(&c.app_id).bind(&meeting_id).bind(&project).bind(&parent).bind(now()).bind(&target.owner).execute(pool).await.map_err(db_error)?;
    } else {
        sqlx::query("UPDATE lark_editable_documents SET status='writing',detail='保存中，未发群' WHERE id=?").bind(&id).execute(pool).await.map_err(db_error)?;
    }
    let result:Result<(),String>=async {
        let doc=if let Some(row)=&existing {row.get::<String,_>("document_id")} else {
            let (path,body)=creation_request(&target,&title);
            let node=app::api(Method::POST,&path,&token,Some(body)).await?;
            let (doc,url)=created_document(&target,&node)?;
            sqlx::query("UPDATE lark_editable_documents SET document_id=?,document_url=?,title=?,status='writing' WHERE id=?")
                .bind(&doc).bind(url).bind(&title).bind(&id).execute(pool).await.map_err(db_error)?;
            previous_revision=Some(remote_revision(&token,&doc).await?);
            doc
        };
        // Insert first: an interrupted update must never erase the last good content.
        // Each mutation carries a specific revision, so concurrent edits cause rejection.
        let revision=previous_revision.ok_or("云文档版本缺失")?;
        let revision=replace_content(&doc,revision,previous_children,content,|method,path,body|{
            let token=&token;async move {app::api(method,&path,token,Some(body)).await}
        }).await?;
        sqlx::query("UPDATE lark_editable_documents SET revision=?,saved_hash=?,saved_content=?,status='ready',detail='已保存到云文档，未发群',updated_at=? WHERE id=?")
            .bind(revision).bind(&expected_hash).bind(markdown).bind(now()).bind(&id).execute(pool).await.map_err(db_error)?;
        Ok(())
    }.await;
    if let Err(error)=result {
        if existing.is_some() && error.starts_with("写入新正文失败：Lark 拒绝请求") {
            sqlx::query("UPDATE lark_editable_documents SET status='ready',detail=? WHERE id=?")
                .bind(&error).bind(&id).execute(pool).await.map_err(db_error)?;
            return Err(format!("云端已拒绝本次更新，未发群，本地草稿仍保留。请核对版本与权限后重试。{error}"));
        }
        // A definite rejection of node creation is safe to retry. Any possible partial
        // write stays blocked, with its link and the previous local content retained.
        if error.starts_with("Lark 拒绝请求") {
            sqlx::query("DELETE FROM lark_editable_documents WHERE id=? AND status='creating' AND document_id IS NULL")
                .bind(&id).execute(pool).await.map_err(db_error)?;
        }
        sqlx::query("UPDATE lark_editable_documents SET status='unknown',detail=? WHERE id=?").bind(&error).bind(&id).execute(pool).await.map_err(db_error)?;
        return Err(format!("云文档保存未确认完成，未发群，本地草稿仍保留。请打开 Lark 核对后处理。{error}"));
    }
    let row=sqlx::query("SELECT * FROM lark_editable_documents WHERE id=?").bind(id).fetch_one(pool).await.map_err(db_error)?;
    let saved=save_title(pool,&token,&target,&row,&title).await?;
    locations::remember(pool,&c,&project).await?;
    Ok(saved)
}

fn validate_title(title:&str)->Result<String,String>{
    let title=title.trim();if title.is_empty()||title.chars().count()>200||title.chars().any(char::is_control){return Err("云文档名称需为 1–200 字，不能包含换行。".into());}Ok(title.into())
}
async fn save_title(pool:&SqlitePool,token:&str,target:&locations::Location,row:&SqliteRow,title:&str)->Result<Value,String>{
    if row.get::<Option<String>,_>("title").as_deref()==Some(title){return Ok(view(row));}
    let doc:String=row.get("document_id");let id:String=row.get("id");
    let revision=remote_revision(token,&doc).await?;
    check_version(&row.get::<String,_>("status"),row.get("revision"),row.get("revision"),revision)?;
    let (method,path,body)=if target.kind=="wiki"{
        let link:String=row.get("document_url");let url=url::Url::parse(&link).map_err(|_|"云文档链接无效")?;
        let node=url.path_segments().and_then(|mut s|s.next_back()).filter(|s|!s.is_empty()&&s.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'_')).ok_or("知识库文档节点无效")?;
        (Method::POST,format!("/open-apis/wiki/v2/spaces/{}/nodes/{node}/update_title",target.space),json!({"title":title}))
    }else{(Method::PATCH,format!("/open-apis/docx/v1/documents/{doc}/blocks/{doc}?document_revision_id={revision}&client_token={}",uuid::Uuid::new_v4()),json!({"update_text_elements":{"elements":[{"text_run":{"content":title}}]}}))};
    sqlx::query("UPDATE lark_editable_documents SET status='renaming',detail='正在更新云文档名称' WHERE id=?").bind(&id).execute(pool).await.map_err(db_error)?;
    let result:Result<i64,String>=async{app::api(method,&path,token,Some(body)).await?;let info=app::api(Method::GET,&format!("/open-apis/docx/v1/documents/{doc}"),token,None).await?;if info["document"]["title"].as_str()!=Some(title){return Err("云端返回的名称与本次修改不一致，请打开 Lark 核对。".into());}info["document"]["revision_id"].as_i64().ok_or("改名后未取得云端版本".into())}.await;
    match result{Ok(next)=>{sqlx::query("UPDATE lark_editable_documents SET title=?,revision=?,status='ready',detail='文档名称和正文已保存，未发群' WHERE id=?").bind(title).bind(next).bind(&id).execute(pool).await.map_err(db_error)?;let row=sqlx::query("SELECT * FROM lark_editable_documents WHERE id=?").bind(id).fetch_one(pool).await.map_err(db_error)?;Ok(view(&row))},Err(e)=>{sqlx::query("UPDATE lark_editable_documents SET status='unknown',detail=? WHERE id=?").bind(&e).bind(id).execute(pool).await.map_err(db_error)?;Err(format!("正文保留，云文档改名未确认完成：{e} 请打开 Lark 核对，未发群。"))}}
}

fn creation_request(target:&locations::Location,title:&str)->(String,Value){
    if target.kind=="drive" {("/open-apis/docx/v1/documents".into(),json!({"folder_token":target.token,"title":title}))}
    else {(format!("/open-apis/wiki/v2/spaces/{}/nodes",target.space),json!({"obj_type":"docx","node_type":"origin","parent_node_token":target.token,"title":title}))}
}
fn created_document(target:&locations::Location,data:&Value)->Result<(String,String),String>{
    let valid=|v:&Value|v.as_str().filter(|s|!s.is_empty()&&s.len()<128&&s.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'_')).map(str::to_owned);
    if target.kind=="drive"{let doc=valid(&data["document"]["document_id"]).ok_or("创建响应缺少文档标识，请核对云空间。")?;Ok((doc.clone(),format!("{}/docx/{doc}",target.origin)))}
    else{let doc=valid(&data["node"]["obj_token"]).ok_or("创建响应缺少文档标识，请核对知识库。")?;let node=valid(&data["node"]["node_token"]).ok_or("知识库节点标识缺失")?;Ok((doc,format!("{}/wiki/{node}",target.origin)))}
}

pub(super) async fn for_send(pool:&SqlitePool,c:&AppConfig,_token:&str,meeting:&str,id:&str,file_hash:&str)->Result<Value,String>{
    let row=sqlx::query("SELECT * FROM lark_editable_documents WHERE id=? AND app_id=? AND meeting_id=?")
        .bind(id).bind(&c.app_id).bind(meeting).fetch_optional(pool).await.map_err(db_error)?.ok_or("云文档关联不存在，请重新打开。")?;
    if row.get::<Option<String>,_>("saved_hash").as_deref()!=Some(file_hash){return Err("本地纪要尚有未同步修改，请先保存到云文档，再预览发送。".into());}
    let doc:Option<String>=row.get("document_id");let doc=doc.ok_or("请先保存云文档。")?;
    let token=locations::access_token(pool,c,&row.get::<String,_>("credential_owner")).await?;
    let remote=remote_revision(&token,&doc).await?;
    check_version(&row.get::<String,_>("status"),row.get("revision"),row.get("revision"),remote)?;
    Ok(view(&row))
}

#[cfg(test)] mod tests {
    use super::*;
    #[test]fn creation_routes_to_the_chosen_storage_without_fallback(){
        let mut target=locations::Location{kind:"drive".into(),token:"folder1".into(),space:String::new(),name:"会议".into(),origin:"https://team.larksuite.com".into(),owner:"ou_a".into()};
        let (path,body)=creation_request(&target,"周会");assert_eq!(path,"/open-apis/docx/v1/documents");assert_eq!(body["folder_token"],"folder1");assert!(body.get("parent_node_token").is_none());
        assert_eq!(created_document(&target,&json!({"document":{"document_id":"doc1"}})).unwrap(),("doc1".into(),"https://team.larksuite.com/docx/doc1".into()));
        target.kind="wiki".into();target.space="space1".into();target.token="node1".into();
        let (path,body)=creation_request(&target,"周会");assert_eq!(path,"/open-apis/wiki/v2/spaces/space1/nodes");assert_eq!(body["parent_node_token"],"node1");assert!(body.get("folder_token").is_none());
        assert!(created_document(&target,&json!({"node":{"obj_token":"doc1","node_token":"bad/path"}})).is_err());
    }
    #[test] fn rejects_remote_changes_stale_windows_and_partial_writes(){
        assert!(check_version("ready",Some(4),Some(4),4).is_ok());
        assert!(check_version("ready",Some(4),Some(4),5).unwrap_err().contains("其他操作"));
        assert!(check_version("ready",Some(4),Some(3),4).unwrap_err().contains("过期"));
        for status in ["creating","writing","unknown"] {assert!(check_version(status,Some(4),Some(4),4).is_err());}
        assert!(check_version("ready",None,None,4).is_err());
    }
    #[tokio::test] async fn inserts_before_deleting_and_carries_revision_to_each_write(){
        let mut calls=Vec::new();
        let revision=replace_content("doc",4,7,json!({"children_id":["new"]}),|method,path,body|{
            calls.push((method,path,body));let revision=4+calls.len() as i64;async move {Ok(json!({"document_revision_id":revision}))}
        }).await.unwrap();
        assert_eq!(revision,6);assert_eq!(calls.len(),2);
        assert_eq!(calls[0].0,Method::POST);assert!(calls[0].1.contains("document_revision_id=4&"));
        assert_eq!(calls[1].0,Method::DELETE);assert!(calls[1].1.contains("document_revision_id=5&"));
        assert_eq!(calls[1].2,json!({"start_index":0,"end_index":7}));
        assert!(calls.iter().all(|(_,path,_)|!path.contains("/im/")));
    }
    #[tokio::test] async fn never_deletes_old_content_after_failed_or_ambiguous_insertion(){
        for response in [Err("network".into()),Ok(json!({}))] {
            let mut calls=0;
            assert!(replace_content("doc",3,5,json!({}),|_,_,_|{calls+=1;let r=response.clone();async move{r}}).await.is_err());
            assert_eq!(calls,1);
        }
    }
    #[tokio::test] async fn initial_save_does_not_delete_and_partial_replace_is_not_success(){
        let mut calls=0;
        assert_eq!(replace_content("doc",1,0,json!({}),|_,_,_|{calls+=1;async{Ok(json!({"document_revision_id":2}))}}).await.unwrap(),2);
        assert_eq!(calls,1);calls=0;
        assert!(replace_content("doc",2,1,json!({}),|_,_,_|{calls+=1;let n=calls;async move{if n==1{Ok(json!({"document_revision_id":3}))}else{Err("revision conflict".into())}}}).await.is_err());
        assert_eq!(calls,2);
    }
    #[tokio::test] async fn binding_is_unique_per_company_meeting_and_project(){
        let pool=SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE meetings(id TEXT PRIMARY KEY)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO meetings VALUES('m')").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261116000000_cloud_document_editor.sql")).execute(&pool).await.unwrap();
        let insert="INSERT INTO lark_editable_documents(id,app_id,meeting_id,project,parent_url,updated_at) VALUES(?,?,'m',?,'url',0)";
        sqlx::query(insert).bind("one").bind("app").bind("项目").execute(&pool).await.unwrap();
        assert!(sqlx::query(insert).bind("two").bind("app").bind("项目").execute(&pool).await.is_err());
        sqlx::query(insert).bind("three").bind("other").bind("项目").execute(&pool).await.unwrap();
    }
}
