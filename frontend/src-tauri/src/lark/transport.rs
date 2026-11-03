// Wire format follows Lark's official oapi-sdk-go/ws protocol (MIT).
// https://github.com/larksuite/oapi-sdk-go/tree/v3_main/ws
use super::app::{self, AppConfig};
use futures_util::{SinkExt, StreamExt};
use prost::Message;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;

#[derive(Clone, PartialEq, Message)]
struct Header {
    #[prost(string, required, tag = "1")]
    key: String,
    #[prost(string, required, tag = "2")]
    value: String,
}
#[derive(Clone, PartialEq, Message)]
struct Frame {
    #[prost(uint64, required, tag = "1")]
    seq_id: u64,
    #[prost(uint64, required, tag = "2")]
    log_id: u64,
    #[prost(int32, required, tag = "3")]
    service: i32,
    #[prost(int32, required, tag = "4")]
    method: i32,
    #[prost(message, repeated, tag = "5")]
    headers: Vec<Header>,
    #[prost(string, optional, tag = "6")]
    encoding: Option<String>,
    #[prost(string, optional, tag = "7")]
    payload_type: Option<String>,
    #[prost(bytes = "vec", optional, tag = "8")]
    payload: Option<Vec<u8>>,
    #[prost(string, optional, tag = "9")]
    log_id_new: Option<String>,
}
impl Frame {
    fn header(&self, key: &str) -> &str {
        self.headers
            .iter()
            .find(|h| h.key == key)
            .map(|h| h.value.as_str())
            .unwrap_or("")
    }
}
struct Parts {
    since: Instant,
    parts: Vec<Option<Vec<u8>>>,
}
#[derive(Default)]
struct Assembler {
    pending: HashMap<String, Parts>,
}
impl Assembler {
    fn push(&mut self, frame: &Frame) -> Result<Option<Vec<u8>>, String> {
        self.pending
            .retain(|_, p| p.since.elapsed() < Duration::from_secs(10));
        let payload = frame.payload.clone().unwrap_or_default();
        if payload.len() > 1_048_576 {
            return Err("Lark 事件过大。".into());
        }
        let total = frame.header("sum").parse::<usize>().unwrap_or(1);
        if total <= 1 {
            return Ok(Some(payload));
        }
        let seq = frame
            .header("seq")
            .parse::<usize>()
            .map_err(|_| "Lark 分片编号无效。")?;
        let id = frame.header("message_id");
        if total > 64 || seq >= total || id.is_empty() || self.pending.len() > 100 {
            return Err("Lark 分片超出限制。".into());
        }
        let entry = self.pending.entry(id.into()).or_insert_with(|| Parts {
            since: Instant::now(),
            parts: vec![None; total],
        });
        if entry.parts.len() != total {
            return Err("Lark 分片数量不一致。".into());
        }
        entry.parts[seq] = Some(payload);
        if entry.parts.iter().flatten().map(Vec::len).sum::<usize>() > 1_048_576 {
            return Err("Lark 合并事件过大。".into());
        }
        if entry.parts.iter().any(Option::is_none) {
            return Ok(None);
        }
        let parts = self.pending.remove(id).unwrap();
        Ok(Some(parts.parts.into_iter().flatten().flatten().collect()))
    }
}

pub(super) async fn run(pool: SqlitePool, config: AppConfig, cancel: CancellationToken) {
    let launch_worker = tokio::spawn(super::launches::run(pool.clone(), cancel.clone()));
    // Meeting polling uses the authenticated user's identity, never all meetings in the tenant.
    let poll_pool = pool.clone();
    let poll_cancel = cancel.clone();
    let poll = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(15));
        loop {
            tokio::select! {_ = poll_cancel.cancelled()=>break,_=interval.tick()=>{
                if let Err(e)=app::match_current(&poll_pool).await {
                    app::status(e).await;
                }
            }}
        }
    });
    loop {
        if cancel.is_cancelled() {
            break;
        }
        app::status("正在连接 Lark…").await;
        let result =
            tokio::select! {_ =cancel.cancelled()=>break,result=connect(&pool,&config)=>result};
        if let Err(e) = result {
            app::status(e).await;
        }
        tokio::select! {_=cancel.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(20))=>{}}
    }
    poll.abort();
    let _ = launch_worker.await;
}

async fn connect(pool: &SqlitePool, config: &AppConfig) -> Result<(), String> {
    let response = app::client()?
        .post(format!("{}/callback/ws/endpoint", app::BASE))
        .json(&json!({"AppID":config.app_id,"AppSecret":config.secret}))
        .send()
        .await
        .map_err(|_| "Lark 长连接初始化网络失败。")?;
    let response: Value = response.json().await.map_err(|_| "Lark 长连接响应异常。")?;
    if response["code"].as_i64() != Some(0) {
        return Err(format!(
            "Lark 长连接错误码 {}，请检查凭证和订阅配置。",
            response["code"]
        ));
    }
    let raw = response["data"]["URL"]
        .as_str()
        .ok_or("Lark 未返回长连接地址。")?;
    let url = url::Url::parse(raw).map_err(|_| "Lark 长连接地址无效。")?;
    if url.scheme() != "wss" || url.host_str().is_none() {
        return Err("Lark 长连接必须使用 TLS。".into());
    }
    let service = url
        .query_pairs()
        .find(|(key, _)| key == "service_id")
        .and_then(|(_, v)| v.parse::<i32>().ok())
        .ok_or("缺少 Lark 服务 ID。")?;
    let ping_secs = response["data"]["ClientConfig"]["PingInterval"]
        .as_u64()
        .unwrap_or(120)
        .clamp(10, 300);
    let (mut ws, _) = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(raw),
    )
    .await
    .map_err(|_| "Lark 长连接建立超时。")?
    .map_err(|_| "无法建立 Lark 长连接。")?;
    super::meeting_state::invalidate();
    app::status("长连接已连接；会议事件即时核验，状态查询共用 60 秒兜底缓存。").await;
    let mut timer = tokio::time::interval(Duration::from_secs(ping_secs));
    let mut assembler = Assembler::default();
    let mut received = Instant::now();
    loop {
        tokio::select! {
            _=timer.tick()=>{
                if received.elapsed()>Duration::from_secs(ping_secs*3){return Err("Lark 心跳超时，正在重连。".into());}
                let frame=Frame{service,headers:vec![Header{key:"type".into(),value:"ping".into()}],..Default::default()};
                ws.send(WsMessage::Binary(frame.encode_to_vec().into())).await.map_err(|_|"Lark 心跳发送失败。")?;
            }
            message=ws.next()=>{
                let Some(Ok(message))=message else{return Err("Lark 连接已断开，正在重连。".into());};
                received=Instant::now();
                match message {
                    WsMessage::Binary(data)=>{
                        let mut frame=Frame::decode(data.as_ref()).map_err(|_|"Lark 帧解析失败。")?;
                        if frame.method!=1||frame.header("type")!="event"{continue;}
                        let Some(payload)=assembler.push(&frame)? else{continue;};
                        let success=match serde_json::from_slice::<Value>(&payload){Ok(event)=>app::receive(pool,config,&event).await.is_ok(),Err(_)=>false};
                        frame.payload=Some(json!({"code":if success{200}else{500},"headers":{},"data":null}).to_string().into_bytes());
                        frame.headers.push(Header{key:"biz_rt".into(),value:"0".into()});
                        ws.send(WsMessage::Binary(frame.encode_to_vec().into())).await.map_err(|_|"Lark 事件确认失败。")?;
                    }
                    WsMessage::Ping(v)=>{ws.send(WsMessage::Pong(v)).await.map_err(|_|"Lark 心跳响应失败。")?;}
                    WsMessage::Close(_)=>return Err("Lark 关闭了长连接，正在重连。".into()),
                    _=>{}
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn part(seq: &str, data: &[u8]) -> Frame {
        Frame {
            headers: vec![
                Header {
                    key: "sum".into(),
                    value: "2".into(),
                },
                Header {
                    key: "seq".into(),
                    value: seq.into(),
                },
                Header {
                    key: "message_id".into(),
                    value: "m".into(),
                },
            ],
            payload: Some(data.to_vec()),
            ..Default::default()
        }
    }
    #[test]
    fn protobuf_and_fragments() {
        let f = part("1", b"b");
        assert_eq!(Frame::decode(f.encode_to_vec().as_slice()).unwrap(), f);
        let mut a = Assembler::default();
        assert_eq!(a.push(&f).unwrap(), None);
        assert_eq!(a.push(&part("0", b"a")).unwrap(), Some(b"ab".to_vec()));
        assert!(a.push(&part("2", b"x")).is_err());
    }
}
