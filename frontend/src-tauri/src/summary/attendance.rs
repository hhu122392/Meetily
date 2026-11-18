use std::{collections::BTreeMap, sync::LazyLock};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use super::meeting_settings::{self, MeetingSummarySettings};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all="camelCase")]
pub struct Attendance {
    pub participants_manual: bool,
    pub absent_manual: bool,
    pub evidence: Vec<Evidence>,
    pub conflicts: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all="camelCase")]
pub struct Evidence { pub name: String, pub status: String, pub quote: String }

static LABEL: LazyLock<Regex> = LazyLock::new(||Regex::new(r"未出席人员|出席人员|参会人员|参与人员|到场人员|在场人员|缺席人员|请假人员|未到人员|主持人|会议时间|会议主题").unwrap());
static SPLIT_NAMES: LazyLock<Regex> = LazyLock::new(||Regex::new(r"\s*(?:[、,，;；\n]|以及|和|及|与|(?i:\s+and\s+))\s*").unwrap());
static NAME: LazyLock<Regex> = LazyLock::new(||Regex::new(r"^(?:[\p{Han}]{2,4}|[A-Za-z][A-Za-z .'-]{0,39})$").unwrap());
static PREFIX: LazyLock<Regex> = LazyLock::new(||Regex::new(r"^(?:(?:本次|这次|今天|今日)(?:会议)?\s*)+").unwrap());
static SUFFIX: LazyLock<Regex> = LazyLock::new(||Regex::new(r"(?P<names>.+?)(?:今天|今日|本次会议|已经|已)?(?P<state>因事请假|请假未到|请假|缺席|未出席|未参会|未到场|没有参加本次会议|已到场|都在场|都在|已参会|出席本次会议|参加本次会议)[了。\s]*$").unwrap());

fn uncertain(text:&str)->bool {
    ["可能","大概","应该","预计","计划","如果","假设","例如","比如","是否","哪些","上次","上周","上个月","昨天","昨日","前天","明天","下次","下周","下个月","以后","将参加","将出席","据说","转述","邀请","应到","模板","请填","填写","写成","标记","不要","不是","未确认","不确定","没有缺席","没有请假","没请假","不缺席","没缺席","未缺席","不请假","未请假","建议","要求","通知","曾经","之前","并不","并非","?","？","\"","“","”","「","」"].iter().any(|word|text.contains(word))
}
fn parse_names(raw:&str)->Vec<String> {
    let raw=raw.trim().trim_matches([':','：','*',' ','-']);
    let raw=raw.strip_prefix("名单").unwrap_or(raw).trim_start_matches([':', '：', ' ']);
    let raw=raw.strip_prefix('为').or_else(||raw.strip_prefix('是')).or_else(||raw.strip_prefix('有')).or_else(||raw.strip_prefix("包括")).unwrap_or(raw).trim_start_matches([':', '：', ' ']);
    let raw=raw.trim_end_matches(['。',' ','.']);
    let raw=["均已到场","均在场","都在场","都到了","都在","在场","等人","等"].iter().find_map(|end|raw.strip_suffix(end)).unwrap_or(raw).trim();
    if raw.is_empty()||uncertain(raw){return Vec::new();}
    let mut result=Vec::new();
    for name in SPLIT_NAMES.split(raw) {
        let name=name.trim().trim_matches('*');
        if !NAME.is_match(name)||["none","unknown","not mentioned","not specified","everyone","nobody","all"].contains(&name.to_lowercase().as_str())||["人员","名单","同事","同学","大家","全体","全部","没有","未提","待定","未定","未知","不详","未记录","没来","没到","未说明","无","未填写","团队","部门","成员","全员","我们","你们","他们","有人","其他","经理","主管","测试","请假","未到","已到","出席","缺席","参加","参会","主持","未能","今天","实际","确认","说"].iter().any(|word|name.contains(word)) {return Vec::new();}
        if !result.iter().any(|n:&String|n.to_lowercase()==name.to_lowercase()){result.push(name.to_owned());}
    }
    if result.len()>80 {Vec::new()}else{result}
}
fn quote(sentence:&str,name:&str)->String{
    let index=sentence.find(name).unwrap_or(0);
    let start=sentence[..index].char_indices().rev().nth(120).map(|(i,_)|i).unwrap_or(0);
    sentence[start..].chars().take(500).collect()
}

/// Conservative extraction from explicit statements, never from speaker identity or a roster.
pub fn extract(text:&str)->Attendance {
    let mut evidence=BTreeMap::<(String,String),Evidence>::new();
    for sentence in text.split_inclusive(['。','！','!','？','?','\n','；',';']) {
        let sentence=sentence.trim();if sentence.is_empty()||uncertain(sentence){continue;}
        let sentence=sentence.trim_end_matches(['。','！','!','\n','；',';']);
        let labels=LABEL.find_iter(sentence).collect::<Vec<_>>();
        if !labels.is_empty() {
            for (index,label) in labels.iter().enumerate(){
                if ["主持人","会议时间","会议主题"].contains(&label.as_str()){continue;}
                let end=labels.get(index+1).map(|m|m.start()).unwrap_or(sentence.len());
                let prefix=&sentence[..label.start()];
                if prefix.ends_with(['不','未','没']){continue;}
                let raw=sentence[label.end()..end].trim().trim_end_matches(['，',',',' ']);
                let status=if ["缺席人员","请假人员","未到人员","未出席人员"].contains(&label.as_str()){"absent"}else{"present"};
                for name in parse_names(raw){let quote=quote(sentence,&name);evidence.entry((name.to_lowercase(),status.into())).or_insert(Evidence{name,status:status.into(),quote});}
            }
        } else {
            for clause in sentence.split(['，',',']) {
                let clause=PREFIX.replace(clause.trim(),"");
                if let Some(caps)=SUFFIX.captures(&clause) {
                    let status=if ["已到场","都在场","都在","已参会","出席本次会议","参加本次会议"].contains(&&caps["state"]){"present"}else{"absent"};
                    for name in parse_names(&caps["names"]){let quote=quote(sentence,&name);evidence.entry((name.to_lowercase(),status.into())).or_insert(Evidence{name,status:status.into(),quote});}
                }
            }
        }
    }
    let mut result=Attendance::default();
    for ((key,_),item) in &evidence {
        if evidence.contains_key(&(key.clone(),if item.status=="present"{"absent".into()}else{"present".into()}))&&!result.conflicts.contains(&item.name){result.conflicts.push(item.name.clone());}
    }
    result.evidence=evidence.into_values().take(160).collect();result
}

fn apply(settings:&mut MeetingSummarySettings,text:&str){
    let previous=settings.attendance.clone().unwrap_or_else(||Attendance{participants_manual:!settings.participants.is_empty(),absent_manual:!settings.absent.is_empty(),..Default::default()});
    let mut found=extract(text);found.participants_manual=previous.participants_manual;found.absent_manual=previous.absent_manual;
    let manual_present=if found.participants_manual{meeting_settings::names(&settings.participants)}else{vec![]};
    let manual_absent=if found.absent_manual{meeting_settings::names(&settings.absent)}else{vec![]};
    let values=|status:&str,opposite:&[String]|found.evidence.iter().filter(|e|e.status==status&&!found.conflicts.iter().any(|n|n.to_lowercase()==e.name.to_lowercase())&&!opposite.iter().any(|n|n.to_lowercase()==e.name.to_lowercase())).map(|e|e.name.clone()).collect::<Vec<_>>().join("、");
    if !found.participants_manual{settings.participants=values("present",&manual_absent);}
    if !found.absent_manual{settings.absent=values("absent",&manual_present);}
    settings.attendance=Some(found);
}

pub async fn refresh_text(pool:&SqlitePool,id:&str,text:&str)->Result<MeetingSummarySettings,String>{
    let _guard=meeting_settings::SETTINGS_LOCK.lock().await;
    let mut settings=meeting_settings::read(pool,id).await?;let previous=settings.clone();
    apply(&mut settings,text);
    if settings!=previous{meeting_settings::save_unlocked(pool,id,&settings).await?;}
    Ok(settings)
}
pub async fn refresh(pool:&SqlitePool,id:&str)->Result<MeetingSummarySettings,String>{
    let rows:Vec<String>=sqlx::query_scalar("SELECT transcript FROM transcripts WHERE meeting_id=? ORDER BY audio_start_time,rowid").bind(id).fetch_all(pool).await.map_err(|e|format!("读取人员识别原文失败：{e}"))?;
    refresh_text(pool,id,&rows.join("\n")).await
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn extracts_explicit_lists_and_current_absence(){
        let mut s=MeetingSummarySettings::default();apply(&mut s,"本次会议缺席人员mike在场人员刘伟、周玲、赵敏都在。张三今天请假。李四和王五已到场。");
        assert_eq!(s.absent,"mike、张三");for name in ["刘伟","周玲","赵敏","李四","王五"]{assert!(s.participants.contains(name));}
        assert!(!s.attendance.as_ref().unwrap().evidence.is_empty());
    }
    #[test]fn silence_roster_history_future_negation_and_conflicts_do_not_imply_attendance(){
        let mut s=MeetingSummarySettings::default();apply(&mut s,"上周张三请假。明天李四参加本次会议。王五没有请假。可能赵敏缺席。参会人员：大家。张三已到场。张三缺席。");
        assert!(s.participants.is_empty());assert!(s.absent.is_empty());assert_eq!(s.attendance.as_ref().unwrap().conflicts,vec!["张三"]);
        apply(&mut s,"讨论张三负责的项目，李四随后提交报告。");assert!(s.participants.is_empty());assert!(s.absent.is_empty());
    }
    #[test]fn manual_values_and_intentionally_empty_fields_survive_reextraction(){
        let mut s=MeetingSummarySettings{participants:"张三".into(),..Default::default()};apply(&mut s,"出席人员：李四。缺席人员：张三、王五。");assert_eq!(s.participants,"张三");assert_eq!(s.absent,"王五");
        s.absent.clear();s.attendance.as_mut().unwrap().absent_manual=true;apply(&mut s,"出席人员：李四。缺席人员：王五。");assert_eq!(s.participants,"张三");assert!(s.absent.is_empty());
    }
    #[test]fn source_correction_replaces_only_automatic_results(){
        let mut s=MeetingSummarySettings::default();apply(&mut s,"出席人员：李四。缺席人员：王五。");assert_eq!(s.participants,"李四");apply(&mut s,"缺席人员：赵敏。");assert!(s.participants.is_empty());assert_eq!(s.absent,"赵敏");
    }
    #[test]fn questions_invitations_and_future_plans_are_not_facts(){
        let result=extract("出席人员：张三？张三将参加本次会议。请填写出席人员：李四。下周缺席人员：王五。邀请人员赵敏。昨日王五缺席。");assert!(result.evidence.is_empty());
    }
    #[test]fn unspecified_people_are_empty_and_already_on_leave_is_not_part_of_the_name(){
        assert!(extract("缺席人员：暂无。参会人员：研发团队。缺席人员：None。我们请假。张三说请假。出席人员：不详。缺席人员：未记录。").evidence.is_empty());
        let mut s=MeetingSummarySettings::default();apply(&mut s,"张三已请假。出席人员：李四和王五，主持人：赵敏。");assert_eq!(s.absent,"张三");assert_eq!(s.participants,"李四、王五");
    }
    #[tokio::test]async fn persisted_manual_clearing_and_parallel_extraction_never_overwrite_each_other(){
        let pool=SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql("CREATE TABLE meetings(id TEXT PRIMARY KEY);INSERT INTO meetings VALUES('a'),('b');CREATE TABLE transcripts(meeting_id TEXT,transcript TEXT,audio_start_time REAL);INSERT INTO transcripts VALUES('a','出席人员：李四。缺席人员：王五。',0);").execute(&pool).await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/20261103000000_meeting_summary_settings.sql")).execute(&pool).await.unwrap();
        let initial=refresh(&pool,"a").await.unwrap();assert_eq!(initial.participants,"李四");assert_eq!(initial.absent,"王五");
        let mut manual=initial.clone();manual.absent.clear();manual.attendance.as_mut().unwrap().absent_manual=true;
        let (saved,extracted)=tokio::join!(meeting_settings::save(&pool,"a",&manual),refresh_text(&pool,"a","出席人员：赵敏。缺席人员：王五。"));saved.unwrap();extracted.unwrap();
        let current=refresh_text(&pool,"a","出席人员：赵敏。缺席人员：王五。").await.unwrap();assert_eq!(current.participants,"赵敏");assert!(current.absent.is_empty());
        // A previously loaded, unedited form must not revert either the manual clearing or new automatic names.
        meeting_settings::save(&pool,"a",&initial).await.unwrap();let current=meeting_settings::read(&pool,"a").await.unwrap();assert_eq!(current.participants,"赵敏");assert!(current.absent.is_empty());
        let other=refresh(&pool,"b").await.unwrap();assert!(other.participants.is_empty()&&other.absent.is_empty());
    }
}
