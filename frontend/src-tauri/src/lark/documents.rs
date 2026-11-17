use super::app::{self, db_error};
use crate::state::AppState;
use serde_json::{json,Value};
use sqlx::Row;
use std::collections::{HashMap, HashSet};

fn token(value:&str)->bool { !value.is_empty() && value.len()<128 && value.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'_') }
pub(super) fn destination(value:&str)->Result<(String,String),String> {
    let url=url::Url::parse(value.trim()).map_err(|_|"请粘贴项目知识库目录页面的 Lark 链接。")?;
    let host=url.host_str().unwrap_or("");
    let segments:Vec<_>=url.path_segments().ok_or("知识库链接无效")?.collect();
    if url.scheme()!="https" || !host.ends_with(".larksuite.com") || !url.username().is_empty() || url.password().is_some() || url.port().is_some()
        || segments.len()!=2 || segments[0]!="wiki" || !token(segments[1]) {
        return Err("请使用 https://公司域名.larksuite.com/wiki/… 格式的项目目录链接。".into());
    }
    Ok((url.origin().ascii_serialization(),segments[1].into()))
}
pub(super) async fn resolve(token_value:&str,parent_url:&str)->Result<Value,String> {
    let (origin,parent)=destination(parent_url)?;
    let data=app::api(reqwest::Method::GET,&format!("/open-apis/wiki/v2/spaces/get_node?token={parent}"),token_value,None).await
        .map_err(|e|format!("无法读取项目知识库，请开通知识库读取权限并授予会议助手目标目录访问权。{e}"))?;
    let node=&data["node"];
    let space=node["space_id"].as_str().filter(|s|token(s)).ok_or("知识库未返回空间标识")?;
    let parent=node["node_token"].as_str().filter(|s|token(s)).ok_or("知识库未返回节点标识")?;
    Ok(json!({"origin":origin,"space":space,"parent":parent,"title":node["title"]}))
}
#[tauri::command]
pub async fn lark_archive_projects(state:tauri::State<'_,AppState>)->Result<Vec<Value>,String>{
    let pool=state.db_manager.pool();let c=app::load(pool).await?;
    let rows=sqlx::query("SELECT name,parent_url FROM lark_archive_projects WHERE app_id=? ORDER BY name").bind(c.app_id).fetch_all(pool).await.map_err(db_error)?;
    Ok(rows.iter().map(|r|json!({"name":r.get::<String,_>("name"),"parentUrl":r.get::<String,_>("parent_url")})).collect())
}
#[tauri::command]
pub async fn lark_archive_project_save(state:tauri::State<'_,AppState>,name:String,parent_url:String)->Result<(),String>{
    let _storage=crate::audio::storage_migration::use_storage().await?;
    let _guard=app::DELIVERY_LOCK.lock().await;
    if name.trim().is_empty()||name.chars().count()>80 {return Err("请填写不超过 80 字的项目名称".into());}
    let (origin,parent)=destination(&parent_url)?;
    let pool=state.db_manager.pool();let c=app::load(pool).await?;
    if c.app_id.is_empty(){return Err("请先配置会议助手".into());}
    sqlx::query("INSERT INTO lark_archive_projects(app_id,name,parent_url) VALUES(?,?,?) ON CONFLICT(app_id,name) DO UPDATE SET parent_url=excluded.parent_url")
        .bind(c.app_id).bind(name.trim()).bind(format!("{origin}/wiki/{parent}")).execute(pool).await.map_err(db_error)?;
    Ok(())
}

// Keep the complete conversion tree: table -> cells -> text, including root order.
// Lark accepts at most 1000 descendants per request. Reject before creating a wiki
// node instead of truncating content or leaving a document with a partial table.
pub(super) fn nested_blocks(converted:&Value)->Result<Value,String>{
    let roots=converted["first_level_block_ids"].as_array().ok_or("Lark 未返回文档块顺序")?;
    let blocks=converted["blocks"].as_array().ok_or("Lark 未返回转换后的文档块")?;
    if roots.is_empty()||blocks.is_empty()||blocks.len()>1000||roots.len()>1000 {
        return Err("云文档内容为空或超过 1000 个内容块（含表格单元格），请拆分后再发送。".into());
    }
    let mut by_id=HashMap::new();
    let mut descendants=Vec::with_capacity(blocks.len());
    for original in blocks {
        let id=original["block_id"].as_str().filter(|s|!s.is_empty()).ok_or("文档块标识缺失")?;
        if by_id.insert(id,original).is_some(){return Err("转换结果包含重复文档块，未创建云文档。".into());}
        match original["block_type"].as_u64() {
            Some(2..=15)|Some(17)|Some(22)|Some(31)|Some(32)|Some(34)=>{},
            Some(27)=>return Err("纪要包含图片，当前云文档归档尚不支持上传图片素材。请移除图片或改用文字链接后再发送。".into()),
            _=>return Err("纪要包含当前归档不支持的文档块，未创建云文档，请调整内容后重试。".into()),
        }
        let mut block=original.clone();
        let object=block.as_object_mut().ok_or("转换后的文档块格式无效")?;
        object.remove("parent_id");object.remove("revision_id");
        if let Some(table)=block.get_mut("table").and_then(Value::as_object_mut) {
            table.remove("cells");
            if let Some(property)=table.get_mut("property").and_then(Value::as_object_mut){
                if let Some(merges)=property.remove("merge_info") {
                    if merges.as_array().is_some_and(|items|items.iter().any(|m|m["row_span"].as_u64().unwrap_or(1)>1||m["col_span"].as_u64().unwrap_or(1)>1)) {
                        return Err("当前云文档归档不支持合并单元格，请拆分单元格后再发送。".into());
                    }
                }
            }
        }
        descendants.push(block);
    }
    let mut pending:Vec<&Value>=roots.iter().collect();
    let mut seen=HashSet::new();
    while let Some(value)=pending.pop(){
        let id=value.as_str().ok_or("文档块引用格式无效")?;
        if !seen.insert(id){return Err("文档块关系重复或循环，未创建云文档。".into());}
        let block=by_id.get(id).ok_or("转换结果缺少子块，未创建云文档。")?;
        if let Some(children)=block.get("children").filter(|v|!v.is_null()) {
            pending.extend(children.as_array().ok_or("文档子块列表格式无效")?.iter());
        }
    }
    if seen.len()!=blocks.len(){return Err("转换结果包含未关联内容，未创建云文档。".into());}
    Ok(json!({"children_id":roots,"descendants":descendants,"index":-1}))
}

#[cfg(test)] mod tests{
    use super::*;
    #[test]fn destinations_reject_untrusted_hosts_and_paths(){
        assert!(destination("https://team.jp.larksuite.com/wiki/Abc123").is_ok());
        for url in ["https://larksuite.com.evil/wiki/Abc","http://team.larksuite.com/wiki/Abc","https://user@team.larksuite.com/wiki/Abc","https://team.larksuite.com/docx/Abc"]{assert!(destination(url).is_err());}
    }
    fn table_fixture()->Value{json!({"first_level_block_ids":["heading","table"],"blocks":[
        {"block_id":"heading","block_type":3,"heading1":{"elements":[{"text_run":{"content":"行动项"}}]}},
        {"block_id":"table","parent_id":"","revision_id":0,"block_type":31,"children":["cell"],"table":{"cells":["cell"],"property":{"row_size":1,"column_size":1,"merge_info":[{"row_span":1,"col_span":1}]}}},
        {"block_id":"cell","block_type":32,"children":["text"],"table_cell":{}},
        {"block_id":"text","block_type":2,"text":{"elements":[{"text_run":{"content":"验收｜甲","text_element_style":{"bold":true}}}]}}
    ]})}
    #[test]fn preserves_native_table_tree_order_and_text_styles(){
        let fixture=table_fixture();let result=nested_blocks(&fixture).unwrap();
        assert_eq!(result["children_id"],json!(["heading","table"]));
        assert_eq!(result["descendants"].as_array().unwrap().len(),4);
        assert_eq!(result["descendants"][1]["children"],json!(["cell"]));
        assert_eq!(result["descendants"][3],fixture["blocks"][3]);
        assert!(result["descendants"][1]["table"].get("cells").is_none());
        assert!(result["descendants"][1]["table"]["property"].get("merge_info").is_none());
        assert!(result["descendants"][1].get("parent_id").is_none());
    }
    #[test]fn rejects_incomplete_or_cyclic_conversion_instead_of_losing_content(){
        for change in 0..5 {
            let mut v=table_fixture();
            match change {
                0=>{v["blocks"].as_array_mut().unwrap().pop();},
                1=>v["blocks"][3]["children"]=json!(["table"]),
                2=>v["first_level_block_ids"]=json!(["heading"]),
                3=>v["blocks"][3]["block_id"]=json!("heading"),
                _=>v["blocks"][3]["children"]=json!("bad"),
            }
            assert!(nested_blocks(&v).is_err());
        }
    }
    #[test]fn rejects_images_merged_cells_empty_and_oversized_conversion(){
        let mut image=table_fixture();image["blocks"][3]["block_type"]=json!(27);
        assert!(nested_blocks(&image).unwrap_err().contains("图片"));
        let mut merged=table_fixture();merged["blocks"][1]["table"]["property"]["merge_info"][0]["col_span"]=json!(2);
        assert!(nested_blocks(&merged).unwrap_err().contains("合并单元格"));
        assert!(nested_blocks(&json!({"first_level_block_ids":[],"blocks":[]})).is_err());
        assert!(nested_blocks(&json!({"first_level_block_ids":["a"],"blocks":vec![json!({});1001]})).is_err());
    }
}
