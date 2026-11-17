use super::app::{self,db_error};
use crate::state::AppState;
use serde_json::{json,Value};
use sqlx::Row;

pub(super) fn render(pattern:&str,values:&[(&str,&str)])->Result<String,String>{
    if pattern.trim().is_empty()||pattern.chars().count()>4000{return Err("消息模板不能为空，最多 4000 字。".into());}
    let mut out=String::new();let mut rest=pattern;
    while let Some(start)=rest.find('{'){
        out.push_str(&rest[..start]);let after=&rest[start+1..];
        let end=after.find('}').ok_or("模板变量缺少右花括号。")?;
        let key=&after[..end];let value=values.iter().find(|(name,_)|*name==key).ok_or_else(||format!("不支持的模板变量：{{{key}}}"))?;
        out.push_str(value.1);rest=&after[end+1..];
    }
    out.push_str(rest);if out.chars().count()>8000{return Err("填入内容后的消息过长，请缩短模板或概要。".into());}Ok(out)
}
fn validate(pattern:&str)->Result<(),String>{render(pattern,&[("会议名称",""),("会议时间",""),("出席人员",""),("缺席人员",""),("主持人",""),("概要",""),("云文档链接","")]).map(|_|())}
#[tauri::command]
pub async fn lark_message_templates(state:tauri::State<'_,AppState>)->Result<Vec<Value>,String>{
    let pool=state.db_manager.pool();let c=app::load(pool).await?;
    let rows=sqlx::query("SELECT name,body FROM lark_message_templates WHERE app_id=? AND owner_id=? ORDER BY name").bind(c.app_id).bind(c.owner_id).fetch_all(pool).await.map_err(db_error)?;
    Ok(rows.iter().map(|r|json!({"name":r.get::<String,_>("name"),"body":r.get::<String,_>("body")})).collect())
}
#[tauri::command]
pub async fn lark_message_template_save(state:tauri::State<'_,AppState>,name:String,body:String)->Result<(),String>{
    let _storage=crate::audio::storage_migration::use_storage().await?;
    let _lock=app::DELIVERY_LOCK.lock().await;validate(&body)?;
    if name.trim().is_empty()||name.chars().count()>80{return Err("请填写不超过 80 字的模板名称。".into());}
    let pool=state.db_manager.pool();let c=app::load(pool).await?;
    sqlx::query("INSERT INTO lark_message_templates(app_id,owner_id,name,body) VALUES(?,?,?,?) ON CONFLICT(app_id,owner_id,name) DO UPDATE SET body=excluded.body")
        .bind(c.app_id).bind(c.owner_id).bind(name.trim()).bind(body).execute(pool).await.map_err(db_error)?;Ok(())
}
#[cfg(test)]mod tests{use super::*;
    #[test]fn validates_placeholders_and_never_expands_user_text_recursively(){assert_eq!(render("{概要}\n{缺席人员}",&[("概要","含{会议名称}的原文"),("缺席人员","甲\n乙")]).unwrap(),"含{会议名称}的原文\n甲\n乙");assert!(validate("{负责人}").is_err());assert!(validate("{概要").is_err());assert!(validate("").is_err());assert!(validate("{会议名称} {云文档链接}").is_ok());}
}
