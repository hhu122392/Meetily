use super::{app::{self,db_error,AppConfig},documents,oauth,payload::fingerprint};
use crate::state::AppState;
use serde::{Serialize,Deserialize};
use serde_json::{json,Value};
use sqlx::{Row,SqlitePool};
use reqwest::Method;
use std::{collections::{HashMap,HashSet,VecDeque},sync::Mutex};
use once_cell::sync::Lazy;
use tokio_util::sync::CancellationToken;

struct Scan { cancel:CancellationToken, requests:usize, loaded:usize }
static SCANS:Lazy<Mutex<HashMap<String,Scan>>>=Lazy::new(||Mutex::new(HashMap::new()));
struct ScanGuard(String);
impl Drop for ScanGuard {fn drop(&mut self){if let Ok(mut scans)=SCANS.lock(){scans.remove(&self.0);}}}
fn tree_scope(c:&AppConfig)->String{fingerprint(&[&c.app_id,&c.owner_id,&c.secret])}
fn tree_key(c:&AppConfig,space:&str)->String{fingerprint(&[&tree_scope(c),space])}

#[derive(Serialize,Deserialize,Clone,Debug)]
#[serde(rename_all="camelCase")]
struct Tree {items:Vec<Value>,complete:bool,notice:String,requests:usize,#[serde(default)]cached:bool,updated_at:i64}

async fn scan_tree<F,Fut,P>(cancel:CancellationToken,mut fetch:F,mut progress:P,max_requests:usize)->Result<Tree,String>
where F:FnMut(String,String)->Fut,Fut:std::future::Future<Output=Result<Value,String>>,P:FnMut(usize,usize) {
    let mut queue=VecDeque::from([(String::new(),Vec::<Value>::new())]);let mut seen=HashSet::new();let mut items=vec![];let mut requests=0;let mut notice=String::new();
    'scan: while let Some((parent,path))=queue.pop_front(){
        let mut cursor=String::new();let mut cursors=HashSet::new();
        loop {
            if cancel.is_cancelled(){notice="已取消，保留本次已加载目录。".into();break 'scan;}
            if requests>=max_requests||items.len()>=5000 {notice="本次已达到加载上限，显示已读取部分；可先搜索这些目录或继续逐级选择。".into();break 'scan;}
            requests+=1;progress(requests,items.len());
            let result=tokio::select!{_ = cancel.cancelled()=>{notice="已取消，保留本次已加载目录。".into();break 'scan;},value=fetch(parent.clone(),cursor.clone())=>value};
            let result=match result{Ok(v)=>v,Err(e)=>{notice=format!("部分目录未能加载，已停止后续请求。{e}");break 'scan;}};
            for node in result["items"].as_array().ok_or("目录树响应格式异常。")? {
                if items.len()>=5000 {notice="本次已达到 5000 个目录，显示已读取部分；可先搜索这些目录或继续逐级选择。".into();break 'scan;}
                let id=node["token"].as_str().filter(|v|!v.is_empty()&&valid_id(v)).ok_or("目录树节点标识无效。")?;
                if !seen.insert(id.to_owned()){continue;}
                let mut child_path=path.clone();child_path.push(node.clone());let mut entry=node.clone();entry["path"]=json!(child_path);items.push(entry);
                if node["hasChildren"]!=false {
                    if child_path.len()<50 {queue.push_back((id.into(),child_path));}else{notice="部分目录超过 50 层，未继续展开。".into();}
                }
            }
            progress(requests,items.len());
            let next=result["nextPageToken"].as_str().ok_or("目录分页格式异常。")?;
            if next.is_empty(){break;}
            if !cursors.insert(next.to_owned()){notice="目录分页未前进，已停止重复请求。".into();break 'scan;}
            cursor=next.into();
        }
    }
    Ok(Tree{items,complete:notice.is_empty(),notice,requests,cached:false,updated_at:app::now()})
}

#[tauri::command]
pub async fn lark_directory_tree(state:tauri::State<'_,AppState>,space:String,scan_id:String,force:bool)->Result<Value,String>{
    if space.is_empty()||!valid_id(&space)||uuid::Uuid::parse_str(&scan_id).is_err(){return Err("请选择知识库后再加载目录。".into());}
    let cancel=CancellationToken::new();
    {let mut scans=SCANS.lock().map_err(|_|"目录加载状态不可用。")?;if !scans.is_empty(){return Err("已有目录正在加载，请先取消或等待完成。".into());}scans.insert(scan_id.clone(),Scan{cancel:cancel.clone(),requests:0,loaded:0});}
    let _scan=ScanGuard(scan_id.clone());let pool=state.db_manager.pool();
    let (c,token)={
        let _config=app::CONFIG_LOCK.lock().await;let c=app::load(pool).await?;
        if !c.enabled||c.app_id.is_empty(){return Err("请先启用会议助手。".into());}
        if !force {
            let cached=sqlx::query("SELECT payload_json,updated_at FROM lark_directory_tree_cache WHERE cache_key=?").bind(tree_key(&c,&space)).fetch_optional(pool).await.map_err(db_error)?;
            if let Some(row)=cached {if (0..1800).contains(&(app::now()-row.get::<i64,_>("updated_at"))) {
                if let Ok(mut value)=serde_json::from_str::<Tree>(&row.get::<String,_>("payload_json")){value.cached=true;return serde_json::to_value(value).map_err(|_|"目录缓存格式错误。".into());}
            }}
        }
        let token=tokio::select!{_ = cancel.cancelled()=>return Err("已取消目录加载。".into()),value=app::tenant_token(&c)=>value?};(c,token)
    };
    let tree=scan_tree(cancel,|parent,cursor|{let token=token.clone();let space=space.clone();async move{
        let data=app::api(Method::GET,&query("wiki",&space,&parent,&cursor)?,&token,None).await?;page("wiki",&space,&data)
    }},|requests,loaded|{if let Ok(mut scans)=SCANS.lock(){if let Some(scan)=scans.get_mut(&scan_id){scan.requests=requests;scan.loaded=loaded;}}},200).await?;
    let _config=app::CONFIG_LOCK.lock().await;let current=app::load(pool).await?;
    if tree_scope(&c)!=tree_scope(&current)||!current.enabled {return Err("加载期间账号或应用已变化，请重新打开目录选择。".into());}
    if tree.complete {sqlx::query("INSERT INTO lark_directory_tree_cache VALUES(?,?,?) ON CONFLICT(cache_key) DO UPDATE SET payload_json=excluded.payload_json,updated_at=excluded.updated_at").bind(tree_key(&c,&space)).bind(serde_json::to_string(&tree).unwrap()).bind(tree.updated_at).execute(pool).await.map_err(db_error)?;}
    serde_json::to_value(tree).map_err(|_|"目录树格式错误。".into())
}
#[tauri::command]
pub fn lark_directory_progress(scan_id:String)->Result<Value,String>{
    let scans=SCANS.lock().map_err(|_|"无法读取加载进度。")?;
    Ok(scans.get(&scan_id).map(|s|json!({"requests":s.requests,"loaded":s.loaded})).unwrap_or(json!({"requests":0,"loaded":0})))
}
#[tauri::command]
pub fn lark_directory_cancel(scan_id:String)->Result<(),String>{
    let scans=SCANS.lock().map_err(|_|"无法取消加载。")?;if let Some(scan)=scans.get(&scan_id){scan.cancel.cancel();}Ok(())
}

#[derive(Clone,Serialize,Deserialize,Debug)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Location {pub kind:String,pub token:String,#[serde(default)]pub space:String,pub name:String,#[serde(default="default_origin")]pub origin:String,#[serde(default)]pub owner:String}
fn default_origin()->String{"https://larksuite.com".into()}
fn valid_id(v:&str)->bool{v.len()<128&&v.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'_')}
impl Location {
    pub(super) fn validate(&self)->Result<(),String>{
        if !matches!(self.kind.as_str(),"wiki"|"drive")||!valid_id(&self.token)||!valid_id(&self.space)||self.name.trim().is_empty()||self.name.chars().count()>500{return Err("保存位置无效，请重新选择目录。".into());}
        if self.kind=="wiki"&&self.space.is_empty(){return Err("请选择知识库。".into());}
        if self.kind=="drive"&&!self.space.is_empty(){return Err("云空间位置无效。".into());}
        let url=url::Url::parse(&self.origin).map_err(|_|"目录域名无效")?;
        let host=url.host_str().unwrap_or("");
        if url.scheme()!="https"||!(host=="larksuite.com"||host.ends_with(".larksuite.com"))||url.origin().ascii_serialization()!=self.origin{return Err("请使用 Lark 云文档目录。".into());}
        Ok(())
    }
    pub(super) fn key(&self)->String{format!("location_{}",fingerprint(&[&self.kind,&self.owner,&self.space,&self.token]))}
    pub(super) fn marker(&self)->String{format!("{}:{}:{}:{}",self.kind,self.owner,self.space,self.token)}
}
// Callers hold CONFIG_LOCK before DELIVERY_LOCK, matching OAuth callback ordering.
pub(super) async fn access_token(pool:&SqlitePool,c:&AppConfig,owner:&str)->Result<String,String>{
    if owner.is_empty(){return app::tenant_token(c).await;}
    if !c.documents_authorized||c.owner_id!=owner {return Err("请连接创建此文档的 Lark 账号的云空间。".into());}
    let refreshed=oauth::authorized_config_locked(pool).await?;
    if refreshed.app_id!=c.app_id||refreshed.owner_id!=owner{return Err("Lark 账号已变化，请重新选择位置。".into());}
    Ok(refreshed.user_token)
}
pub(super) async fn load(pool:&SqlitePool,c:&AppConfig,project:&str)->Result<(Location,String),String>{
    let row=sqlx::query("SELECT * FROM lark_archive_projects WHERE app_id=? AND name=? AND (owner_id='' OR owner_id=?)").bind(&c.app_id).bind(project).bind(&c.owner_id).fetch_optional(pool).await.map_err(db_error)?.ok_or("保存位置不存在，请重新选择。")?;
    if let Some(raw)=row.get::<Option<String>,_>("target_json") {
        let target:Location=serde_json::from_str(&raw).map_err(|_|"保存位置记录无效")?;target.validate()?;
        let marker=target.marker();Ok((target,marker))
    }else{
        let parent:String=row.get("parent_url");let token=app::tenant_token(c).await?;
        let resolved=documents::resolve(&token,&parent).await?;
        Ok((Location{kind:"wiki".into(),token:resolved["parent"].as_str().ok_or("目录无效")?.into(),space:resolved["space"].as_str().ok_or("知识库无效")?.into(),name:project.into(),origin:resolved["origin"].as_str().ok_or("域名无效")?.into(),owner:String::new()},parent))
    }
}
pub(super) async fn remember(pool:&SqlitePool,c:&AppConfig,project:&str)->Result<(),String>{
    sqlx::query("INSERT INTO lark_cloud_preferences(app_id,owner_id,last_project) VALUES(?,?,?) ON CONFLICT(app_id,owner_id) DO UPDATE SET last_project=excluded.last_project").bind(&c.app_id).bind(&c.owner_id).bind(project).execute(pool).await.map_err(db_error)?;
    sqlx::query("UPDATE lark_archive_projects SET last_used_at=? WHERE app_id=? AND name=? AND (owner_id='' OR owner_id=?)").bind(app::now()).bind(&c.app_id).bind(project).bind(&c.owner_id).execute(pool).await.map_err(db_error)?;Ok(())
}
#[tauri::command]
pub async fn lark_location_context(state:tauri::State<'_,AppState>)->Result<Value,String>{
    let pool=state.db_manager.pool();let c=app::load(pool).await?;
    let rows=sqlx::query("SELECT name,display_name,parent_url,target_json FROM lark_archive_projects WHERE app_id=? AND (owner_id='' OR owner_id=?) ORDER BY last_used_at DESC,COALESCE(display_name,name)").bind(&c.app_id).bind(&c.owner_id).fetch_all(pool).await.map_err(db_error)?;
    let last:Option<String>=sqlx::query_scalar("SELECT last_project FROM lark_cloud_preferences WHERE app_id=? AND owner_id=?").bind(&c.app_id).bind(&c.owner_id).fetch_optional(pool).await.map_err(db_error)?.flatten();
    Ok(json!({"projects":rows.iter().map(|r|json!({"name":r.get::<String,_>("name"),"label":r.get::<Option<String>,_>("display_name").unwrap_or_else(||r.get("name")),"parentUrl":r.get::<String,_>("parent_url")})).collect::<Vec<_>>(),"lastProject":last,"cacheScope":tree_scope(&c),"driveConnected":c.documents_authorized&&!c.user_token.is_empty()}))
}
fn query(kind:&str,space:&str,parent:&str,page:&str)->Result<String,String>{
    if !valid_id(space)||!valid_id(parent)||page.len()>4096{return Err("目录参数无效".into());}
    let mut q=url::form_urlencoded::Serializer::new(String::new());q.append_pair("page_size",if kind=="drive"{"200"}else{"50"});
    if !page.is_empty(){q.append_pair("page_token",page);}
    let path=match kind{
        "spaces"=>"/open-apis/wiki/v2/spaces".into(),
        "wiki" if !space.is_empty()=>{if !parent.is_empty(){q.append_pair("parent_node_token",parent);}format!("/open-apis/wiki/v2/spaces/{space}/nodes")},
        "drive"=>{if !parent.is_empty(){q.append_pair("folder_token",parent);} "/open-apis/drive/v1/files".into()},
        _=>return Err("请选择云空间或知识库。".into())
    };Ok(format!("{path}?{}",q.finish()))
}
fn page(kind:&str,space:&str,data:&Value)->Result<Value,String>{
    if !data.is_object(){return Err("目录响应缺少数据，请刷新重试。".into());}
    let source=if kind=="drive"{"files"}else{"items"};
    let empty=Vec::new();let rows=match data.get(source){Some(Value::Array(v))=>v,None|Some(Value::Null)=>&empty,_=>return Err("目录列表格式异常，未更新目录。".into())};
    let mut items=Vec::new();
    for row in rows{
        if kind=="drive"&&row["type"]!="folder"{continue;}
        // Shortcuts may point outside the selected space; do not offer them as containers.
        if kind=="wiki"&&row["node_type"]=="shortcut"{continue;}
        let id=row[if kind=="spaces"{"space_id"}else if kind=="drive"{"token"}else{"node_token"}].as_str().filter(|v|!v.is_empty()&&valid_id(v)).ok_or("目录标识缺失")?;
        let name=row[if kind=="wiki"{"title"}else{"name"}].as_str().unwrap_or("未命名目录");
        let origin=row["url"].as_str().and_then(|v|url::Url::parse(v).ok()).map(|u|u.origin().ascii_serialization()).unwrap_or_else(default_origin);
        items.push(json!({"token":id,"name":name,"space":if kind=="spaces"{id}else{space},"origin":origin,"hasChildren":row["has_child"].as_bool()}));
    }
    let more=data["has_more"].as_bool().unwrap_or(false);
    let next=if more{data[if kind=="drive"{"next_page_token"}else{"page_token"}].as_str().filter(|v|!v.is_empty()).ok_or("目录分页标识缺失")?}else{""};
    Ok(json!({"items":items,"nextPageToken":next}))
}
#[tauri::command]
pub async fn lark_directory_list(state:tauri::State<'_,AppState>,kind:String,space:String,parent:String,page_token:String)->Result<Value,String>{
    let _config=app::CONFIG_LOCK.lock().await;let pool=state.db_manager.pool();let c=app::load(pool).await?;
    let owner=if kind=="drive"{if !c.documents_authorized||c.owner_id.is_empty(){return Err("请先点击“连接我的云空间”完成 Lark 授权。".into());}&c.owner_id}else{""};
    let token=access_token(pool,&c,owner).await?;
    let data=app::api(Method::GET,&query(&kind,&space,&parent,&page_token)?,&token,None).await.map_err(|e|format!("无法读取目录。云空间需用户身份 drive:drive:readonly；知识库需应用身份 wiki:wiki:readonly（或 wiki:wiki），并具有目录访问权。{e}"))?;
    page(&kind,&space,&data)
}
#[tauri::command]
pub async fn lark_location_choose(state:tauri::State<'_,AppState>,mut target:Location)->Result<String,String>{
    let _storage=crate::audio::storage_migration::use_storage().await?;
    let _config=app::CONFIG_LOCK.lock().await;let _send=app::DELIVERY_LOCK.lock().await;
    let pool=state.db_manager.pool();let c=app::load(pool).await?;target.validate()?;
    target.owner=if target.kind=="drive"{if !c.documents_authorized||c.owner_id.is_empty(){return Err("请连接我的云空间。".into());}c.owner_id.clone()}else{String::new()};
    let token=access_token(pool,&c,&target.owner).await?;
    app::api(Method::GET,&query(&target.kind,&target.space,&target.token,"")?,&token,None).await?;
    let key=target.key();let raw=serde_json::to_string(&target).map_err(|_|"目录数据无效")?;
    sqlx::query("INSERT INTO lark_archive_projects(app_id,name,parent_url,target_json,owner_id,display_name,last_used_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(app_id,name) DO UPDATE SET target_json=excluded.target_json,display_name=excluded.display_name,last_used_at=excluded.last_used_at")
        .bind(&c.app_id).bind(&key).bind(target.marker()).bind(raw).bind(&target.owner).bind(&target.name).bind(app::now()).execute(pool).await.map_err(db_error)?;
    Ok(key)
}
#[cfg(test)] mod tests{
    use super::*;
    #[tokio::test]async fn whole_tree_follows_empty_pages_and_only_expands_nodes_with_children(){
        let calls=std::sync::Arc::new(Mutex::new(Vec::new()));let captured=calls.clone();
        let tree=scan_tree(CancellationToken::new(),move|parent,cursor|{
            captured.lock().unwrap().push((parent.clone(),cursor.clone()));async move{
                Ok(match (parent.as_str(),cursor.as_str()){
                    ("","")=>json!({"items":[],"nextPageToken":"page2"}),
                    ("","page2")=>json!({"items":[{"token":"a","name":"项目","hasChildren":true},{"token":"b","name":"无子页","hasChildren":false}],"nextPageToken":""}),
                    ("a","")=>json!({"items":[{"token":"c","name":"战略会","hasChildren":false}],"nextPageToken":""}),
                    _=>panic!("叶子不应继续读取")
                })
            }
        },|_,_|{},200).await.unwrap();
        assert!(tree.complete);assert_eq!(tree.requests,3);assert_eq!(tree.items.len(),3);assert_eq!(tree.items[2]["path"][0]["token"],"a");assert_eq!(tree.items[2]["path"][1]["token"],"c");assert_eq!(calls.lock().unwrap().len(),3);
    }
    #[tokio::test]async fn cancellation_and_errors_preserve_partial_results_without_more_requests(){
        let cancel=CancellationToken::new();let signal=cancel.clone();
        let tree=scan_tree(cancel,move|_,_|{signal.cancel();async{Ok(json!({"items":[{"token":"a","hasChildren":true}],"nextPageToken":"next"}))}},|_,_|{},200).await.unwrap();
        assert!(!tree.complete);assert_eq!(tree.requests,1);assert!(tree.notice.contains("取消"));
        let failed=scan_tree(CancellationToken::new(),|parent,_|async move{if parent.is_empty(){Ok(json!({"items":[{"token":"a","hasChildren":true}],"nextPageToken":""}))}else{Err("目录无权限".into())}},|_,_|{},200).await.unwrap();
        assert!(!failed.complete);assert_eq!(failed.items.len(),1);assert_eq!(failed.requests,2);assert!(failed.notice.contains("无权限"));
        let stopped=CancellationToken::new();stopped.cancel();let tree=scan_tree(stopped,|_,_|async{panic!("取消后不能发请求")},|_,_|{},200).await.unwrap();assert_eq!(tree.requests,0);
    }
    #[tokio::test]async fn pagination_loops_and_request_limits_cannot_trigger_endless_reads(){
        let tree=scan_tree(CancellationToken::new(),|_,_|async{Ok(json!({"items":[],"nextPageToken":"same"}))},|_,_|{},200).await.unwrap();assert!(!tree.complete);assert_eq!(tree.requests,2);
        let tree=scan_tree(CancellationToken::new(),|_,_|async{Ok(json!({"items":[{"token":"a","hasChildren":true}],"nextPageToken":""}))},|_,_|{},1).await.unwrap();assert!(!tree.complete);assert_eq!(tree.requests,1);assert_eq!(tree.items.len(),1);
        let a=AppConfig{app_id:"cli_a".into(),owner_id:"ou_a".into(),secret:"secret_a".into(),..Default::default()};
        assert_ne!(tree_key(&a,"s"),tree_key(&AppConfig{owner_id:"ou_b".into(),..a.clone()},"s"));assert_ne!(tree_key(&a,"s"),tree_key(&AppConfig{secret:"changed".into(),..a.clone()},"s"));assert_ne!(tree_key(&a,"s"),tree_key(&a,"other"));
    }
    #[tokio::test]async fn successful_location_preferences_are_scoped_to_app_and_account(){
        let pool=SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE lark_archive_projects(app_id TEXT,name TEXT,parent_url TEXT); CREATE TABLE lark_editable_documents(id TEXT);").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261116010000_cloud_locations_templates.sql")).execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261116040000_directory_tree_cache.sql")).execute(&pool).await.unwrap();
        let mut a=AppConfig::default();a.app_id="app".into();a.owner_id="a".into();
        let mut b=a.clone();b.owner_id="b".into();
        remember(&pool,&a,"wiki").await.unwrap();remember(&pool,&b,"drive").await.unwrap();remember(&pool,&a,"new-wiki").await.unwrap();
        let rows:Vec<(String,String)>=sqlx::query_as("SELECT owner_id,last_project FROM lark_cloud_preferences ORDER BY owner_id").fetch_all(&pool).await.unwrap();
        assert_eq!(rows,vec![("a".into(),"new-wiki".into()),("b".into(),"drive".into())]);
    }
    #[test]fn paginated_folder_filter_keeps_continuation_even_when_empty(){let p=page("drive","",&json!({"files":[{"type":"docx"}],"has_more":true,"next_page_token":"next"})).unwrap();assert_eq!(p["items"],json!([]));assert_eq!(p["nextPageToken"],"next");assert!(page("wiki","s",&json!({"items":[],"has_more":true})).is_err());}
    #[test]fn locations_are_account_and_space_specific(){let a=Location{kind:"drive".into(),token:"folder".into(),space:String::new(),name:"目录".into(),origin:default_origin(),owner:"ou_a".into()};let mut b=a.clone();b.owner="ou_b".into();assert_ne!(a.key(),b.key());assert!(a.validate().is_ok());b.origin="https://larksuite.com.evil".into();assert!(b.validate().is_err());assert!(query("wiki","x?bad","","").is_err());assert!(query("drive","","","x&z").unwrap().contains("x%26z"));}
}
