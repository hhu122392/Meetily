use std::io::{BufRead, Write};

use serde::{Deserialize, Serialize};

// R3 carries the exact native decode contract in both directions. Bump the
// protocol so a helper that cannot prove its decode language is rejected.
pub const PROTOCOL_VERSION: u16 = 3;
pub const MAX_JSONL_BYTES: usize = 256 * 1024;
// JSON control characters can expand to six bytes (for example, `\u0000`).
// This ceiling leaves enough room for that worst case plus the JSON envelope.
pub const MAX_TEXT_CHUNK_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Probe(ProbeCommand),
    Transcribe(TranscribeCommand),
    Cancel(CancelCommand),
    Shutdown(ShutdownCommand),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProbeCommand {
    pub v: u16,
    pub request_id: String,
    pub client_seq: u64,
    pub runtime: RuntimeSpec,
    pub device: DeviceSpec,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TranscribeCommand {
    pub v: u16,
    pub request_id: String,
    pub client_seq: u64,
    pub context_sha256: String,
    pub runtime: RuntimeSpec,
    pub device: DeviceSpec,
    pub model: ModelSpec,
    pub audio: AudioSpec,
    pub language_requested: String,
    pub decode_parameters_json: String,
    pub decode_parameters_sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CancelCommand {
    pub v: u16,
    pub request_id: String,
    pub client_seq: u64,
    pub reason: CancelReason,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ShutdownCommand {
    pub v: u16,
    pub request_id: String,
    pub client_seq: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSpec {
    pub directory: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeviceSpec {
    pub kind: String,
    pub description: String,
    pub device_id: Option<String>,
    pub allow_primary_fallback: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AudioSpec {
    pub path: String,
    pub format: String,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub samples: u64,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CancelReason {
    User,
    Timeout,
    Shutdown,
}

impl ClientMessage {
    pub fn v(&self) -> u16 {
        match self {
            Self::Probe(value) => value.v,
            Self::Transcribe(value) => value.v,
            Self::Cancel(value) => value.v,
            Self::Shutdown(value) => value.v,
        }
    }

    pub fn request_id(&self) -> &str {
        match self {
            Self::Probe(value) => &value.request_id,
            Self::Transcribe(value) => &value.request_id,
            Self::Cancel(value) => &value.request_id,
            Self::Shutdown(value) => &value.request_id,
        }
    }

    pub fn client_seq(&self) -> u64 {
        match self {
            Self::Probe(value) => value.client_seq,
            Self::Transcribe(value) => value.client_seq,
            Self::Cancel(value) => value.client_seq,
            Self::Shutdown(value) => value.client_seq,
        }
    }

    pub fn validate(&self, expected_client_seq: u64) -> Result<(), ProtocolError> {
        if self.v() != PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion);
        }
        if !is_uuid(self.request_id()) {
            return Err(ProtocolError::InvalidRequestId);
        }
        if self.client_seq() != expected_client_seq {
            return Err(ProtocolError::InvalidClientSeq);
        }
        if let Self::Transcribe(command) = self {
            if !is_sha256(&command.context_sha256) {
                return Err(ProtocolError::InvalidContextHash);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    ProtocolLineTooLong,
    ProtocolInvalidJson,
    ProtocolUnsupportedVersion,
    ProtocolInvalidRequestId,
    ProtocolInvalidClientSeq,
    ProtocolUnexpectedMessage,
    ProtocolInvalidContextHash,
    ProtocolEof,
    ProtocolIo,
    RuntimeUnsupportedPlatform,
    RuntimeEnvironmentForbidden,
    RuntimeContractMissing,
    RuntimeContractMismatch,
    RuntimeHashMismatch,
    RuntimeLoadFailed,
    RuntimeSymbolMissing,
    RuntimeVersionMismatch,
    RuntimeCommitMismatch,
    RuntimeAbiMismatch,
    BackendInitFailed,
    DevicePolicyRejected,
    DeviceNotFound,
    DeviceAmbiguous,
    DeviceFallbackForbidden,
    ModelContractMismatch,
    ModelLoadFailed,
    SessionInitFailed,
    AudioContractMismatch,
    AudioHashMismatch,
    AudioNonFinite,
    AudioOutOfRange,
    NativeRunFailed,
    NativeOutputInvalid,
    NativeOutputTruncated,
    Cancelled,
    Internal,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Hello(HelloMessage),
    Accepted(AcceptedMessage),
    Status(StatusMessage),
    Heartbeat(HeartbeatMessage),
    ProbeResult(ProbeResultMessage),
    TextChunk(TextChunkMessage),
    Segment(SegmentMessage),
    Completed(CompletedMessage),
    Cancelled(CancelledMessage),
    Shutdown(ShutdownMessage),
    Failed(FailedMessage),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HelloMessage {
    pub v: u16,
    pub seq: u64,
    pub helper_version: String,
    pub pid: u32,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AcceptedMessage {
    pub v: u16,
    pub seq: u64,
    pub request_id: String,
    pub operation: Operation,
    pub context_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Probe,
    Transcribe,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    Preflight,
    NativeRunning,
    ResultStreaming,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StatusMessage {
    pub v: u16,
    pub seq: u64,
    pub request_id: String,
    pub context_sha256: String,
    pub phase: TaskPhase,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HeartbeatMessage {
    pub v: u16,
    pub seq: u64,
    pub request_id: String,
    pub context_sha256: String,
    pub phase: TaskPhase,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProbeResultMessage {
    pub v: u16,
    pub seq: u64,
    pub request_id: String,
    pub terminal: bool,
    pub runtime_version: String,
    pub runtime_commit: String,
    pub abi_verified: bool,
    pub backend: String,
    pub device: DeviceResult,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeviceResult {
    pub kind: String,
    pub description: String,
    pub device_id: Option<String>,
    pub device_type: String,
    pub memory_total_bytes: u64,
    pub memory_free_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TextChunkMessage {
    pub v: u16,
    pub seq: u64,
    pub request_id: String,
    pub stream: TextStream,
    pub part: u32,
    pub last: bool,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextStream {
    Raw,
    Clean,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SegmentMessage {
    pub v: u16,
    pub seq: u64,
    pub request_id: String,
    pub segment_index: u32,
    pub t0_ms: i64,
    pub t1_ms: i64,
    pub speaker_id: i32,
    pub text_part: u32,
    pub text_last: bool,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CompletedMessage {
    pub v: u16,
    pub seq: u64,
    pub request_id: String,
    pub context_sha256: String,
    pub terminal: bool,
    pub status: CompletionStatus,
    pub backend: String,
    pub device_description: String,
    pub native_run_elapsed_ms: u64,
    pub native_rtf: f64,
    pub wall_elapsed_ms: u64,
    pub wall_rtf: f64,
    pub last_timestamp_ms: i64,
    pub segment_count: u32,
    pub raw_text_sha256: String,
    pub clean_text_sha256: String,
    pub was_aborted: bool,
    pub was_truncated: bool,
    pub native_session_limits: NativeSessionLimits,
    pub native_timings: NativeTimings,
    pub language_requested: String,
    pub language_resolved: String,
    pub decode_parameters_json: String,
    pub decode_parameters_sha256: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompletionStatus {
    Ok,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NativeTimings {
    pub load_ms: f32,
    pub mel_ms: f32,
    pub encode_ms: f32,
    pub decode_ms: f32,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeSessionLimits {
    pub effective_n_ctx: i32,
    pub effective_max_audio_ms: i64,
    pub max_kv_bytes: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CancelledMessage {
    pub v: u16,
    pub seq: u64,
    pub request_id: String,
    pub terminal: bool,
    pub partial_result_available: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ShutdownMessage {
    pub v: u16,
    pub seq: u64,
    pub request_id: String,
    pub terminal: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FailedMessage {
    pub v: u16,
    pub seq: u64,
    pub request_id: Option<String>,
    pub terminal: bool,
    pub code: ErrorCode,
    pub phase: String,
    pub retryable: bool,
    pub native_status: Option<i32>,
    pub message: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("JSONL line exceeds the protocol limit")]
    LineTooLong,
    #[error("invalid JSONL message")]
    InvalidJson,
    #[error("unsupported protocol version")]
    UnsupportedVersion,
    #[error("invalid request identifier")]
    InvalidRequestId,
    #[error("invalid client sequence")]
    InvalidClientSeq,
    #[error("invalid context hash")]
    InvalidContextHash,
    #[error("input stream closed")]
    Eof,
    #[error("protocol I/O failed")]
    Io,
}

impl ProtocolError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::LineTooLong => ErrorCode::ProtocolLineTooLong,
            Self::InvalidJson => ErrorCode::ProtocolInvalidJson,
            Self::UnsupportedVersion => ErrorCode::ProtocolUnsupportedVersion,
            Self::InvalidRequestId => ErrorCode::ProtocolInvalidRequestId,
            Self::InvalidClientSeq => ErrorCode::ProtocolInvalidClientSeq,
            Self::InvalidContextHash => ErrorCode::ProtocolInvalidContextHash,
            Self::Eof => ErrorCode::ProtocolEof,
            Self::Io => ErrorCode::ProtocolIo,
        }
    }
}

pub fn read_client_message<R: BufRead>(reader: &mut R) -> Result<ClientMessage, ProtocolError> {
    let line = read_limited_line(reader)?;
    serde_json::from_slice(&line).map_err(|_| ProtocolError::InvalidJson)
}

pub fn read_server_message<R: BufRead>(reader: &mut R) -> Result<ServerMessage, ProtocolError> {
    let line = read_limited_line(reader)?;
    serde_json::from_slice(&line).map_err(|_| ProtocolError::InvalidJson)
}

pub fn write_server_message<W: Write>(
    writer: &mut W,
    message: &ServerMessage,
) -> Result<(), ProtocolError> {
    let encoded = serde_json::to_vec(message).map_err(|_| ProtocolError::InvalidJson)?;
    if encoded.len() + 1 > MAX_JSONL_BYTES {
        return Err(ProtocolError::LineTooLong);
    }
    writer.write_all(&encoded).map_err(|_| ProtocolError::Io)?;
    writer.write_all(b"\n").map_err(|_| ProtocolError::Io)?;
    writer.flush().map_err(|_| ProtocolError::Io)
}

pub fn write_client_message<W: Write>(
    writer: &mut W,
    message: &ClientMessage,
) -> Result<(), ProtocolError> {
    let encoded = serde_json::to_vec(message).map_err(|_| ProtocolError::InvalidJson)?;
    if encoded.len() + 1 > MAX_JSONL_BYTES {
        return Err(ProtocolError::LineTooLong);
    }
    writer.write_all(&encoded).map_err(|_| ProtocolError::Io)?;
    writer.write_all(b"\n").map_err(|_| ProtocolError::Io)?;
    writer.flush().map_err(|_| ProtocolError::Io)
}

fn read_limited_line<R: BufRead>(reader: &mut R) -> Result<Vec<u8>, ProtocolError> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(|_| ProtocolError::Io)?;
        if available.is_empty() {
            return if line.is_empty() {
                Err(ProtocolError::Eof)
            } else {
                Ok(trim_cr(line))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(take) > MAX_JSONL_BYTES {
            return Err(ProtocolError::LineTooLong);
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            line.pop();
            return Ok(trim_cr(line));
        }
    }
}

fn trim_cr(mut value: Vec<u8>) -> Vec<u8> {
    if value.last() == Some(&b'\r') {
        value.pop();
    }
    value
}

pub fn utf8_chunks(value: &str) -> Vec<&str> {
    if value.is_empty() {
        return vec![""];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < value.len() {
        let mut end = (start + MAX_TEXT_CHUNK_BYTES).min(value.len());
        while end > start && !value.is_char_boundary(end) {
            end -= 1;
        }
        debug_assert!(end > start);
        chunks.push(&value[start..end]);
        start = end;
    }
    chunks
}

pub fn is_uuid(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    value.bytes().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => byte == b'-',
        _ => byte.is_ascii_hexdigit(),
    })
}

pub fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    const REQUEST_ID: &str = "798d8c63-5ff1-40e3-9db8-0f706aeb930a";

    fn probe_json(extra: &str) -> String {
        format!(
            "{{\"type\":\"probe\",\"v\":{PROTOCOL_VERSION},\"request_id\":\"{REQUEST_ID}\",\"client_seq\":1,\"runtime\":{{\"directory\":\"runtime\"}},\"device\":{{\"kind\":\"vulkan\",\"description\":\"Intel(R) Arc(TM) Graphics\",\"device_id\":null,\"allow_primary_fallback\":false}}{extra}}}\n"
        )
    }

    fn transcribe_json() -> serde_json::Value {
        serde_json::json!({
            "type": "transcribe",
            "v": PROTOCOL_VERSION,
            "request_id": REQUEST_ID,
            "client_seq": 1,
            "context_sha256": "a".repeat(64),
            "runtime": { "directory": "runtime" },
            "device": {
                "kind": "vulkan",
                "description": "Intel(R) Arc(TM) Graphics",
                "device_id": null,
                "allow_primary_fallback": false
            },
            "model": { "path": "model", "bytes": 1, "sha256": "b".repeat(64) },
            "audio": {
                "path": "audio",
                "format": "f32le",
                "sample_rate_hz": 16000,
                "channels": 1,
                "samples": 16000,
                "bytes": 64000,
                "sha256": "c".repeat(64)
            },
            "language_requested": "zh-CN",
            "decode_parameters_json": "{\"language\":\"zh\",\"timestamps\":\"segment\",\"diarize\":\"on\"}",
            "decode_parameters_sha256": "d".repeat(64)
        })
    }

    #[test]
    fn parses_and_validates_probe() {
        let mut reader = BufReader::new(Cursor::new(probe_json("")));
        let message = read_client_message(&mut reader).unwrap();
        message.validate(1).unwrap();
        assert!(matches!(message, ClientMessage::Probe(_)));
    }

    #[test]
    fn rejects_unknown_fields() {
        let mut reader = BufReader::new(Cursor::new(probe_json(",\"surprise\":true")));
        assert_eq!(
            read_client_message(&mut reader).unwrap_err(),
            ProtocolError::InvalidJson
        );
    }

    #[test]
    fn transcribe_request_rejects_each_missing_decode_contract_field() {
        for field in [
            "language_requested",
            "decode_parameters_json",
            "decode_parameters_sha256",
        ] {
            let mut value = transcribe_json();
            value.as_object_mut().unwrap().remove(field);
            let input = format!("{}\n", serde_json::to_string(&value).unwrap());
            let mut reader = BufReader::new(Cursor::new(input));
            assert_eq!(
                read_client_message(&mut reader).unwrap_err(),
                ProtocolError::InvalidJson,
                "missing {field} must fail closed"
            );
        }
    }

    #[test]
    fn rejects_oversized_line_before_json_parsing() {
        let bytes = vec![b'x'; MAX_JSONL_BYTES + 1];
        let mut reader = BufReader::new(Cursor::new(bytes));
        assert_eq!(
            read_client_message(&mut reader).unwrap_err(),
            ProtocolError::LineTooLong
        );
    }

    #[test]
    fn validates_version_request_id_and_sequence() {
        let mut reader = BufReader::new(Cursor::new(probe_json("")));
        let mut message = read_client_message(&mut reader).unwrap();
        assert_eq!(
            message.validate(2).unwrap_err(),
            ProtocolError::InvalidClientSeq
        );
        if let ClientMessage::Probe(probe) = &mut message {
            probe.v = PROTOCOL_VERSION + 1;
        }
        assert_eq!(
            message.validate(1).unwrap_err(),
            ProtocolError::UnsupportedVersion
        );
    }

    #[test]
    fn chunks_unicode_without_breaking_utf8() {
        let value = format!("{}尾", "中".repeat(MAX_TEXT_CHUNK_BYTES / 3 + 3));
        let chunks = utf8_chunks(&value);
        assert!(chunks.len() >= 2);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.len() <= MAX_TEXT_CHUNK_BYTES));
        assert_eq!(chunks.concat(), value);
    }

    #[test]
    fn serialized_message_is_one_physical_line() {
        let message = ServerMessage::TextChunk(TextChunkMessage {
            v: PROTOCOL_VERSION,
            seq: 1,
            request_id: REQUEST_ID.to_string(),
            stream: TextStream::Clean,
            part: 0,
            last: true,
            text: "first\nsecond".to_string(),
        });
        let mut output = Vec::new();
        write_server_message(&mut output, &message).unwrap();
        assert_eq!(output.iter().filter(|byte| **byte == b'\n').count(), 1);
        assert!(String::from_utf8(output)
            .unwrap()
            .contains("first\\nsecond"));
    }

    #[test]
    fn server_messages_round_trip_strictly() {
        let message = ServerMessage::Shutdown(ShutdownMessage {
            v: PROTOCOL_VERSION,
            seq: 2,
            request_id: REQUEST_ID.to_string(),
            terminal: true,
        });
        let mut output = Vec::new();
        write_server_message(&mut output, &message).unwrap();
        let mut reader = BufReader::new(Cursor::new(output));
        assert_eq!(read_server_message(&mut reader).unwrap(), message);

        let invalid = format!(
            "{{\"type\":\"shutdown\",\"v\":{PROTOCOL_VERSION},\"seq\":2,\"request_id\":\"{REQUEST_ID}\",\"terminal\":true,\"extra\":1}}\n"
        );
        let mut reader = BufReader::new(Cursor::new(invalid));
        assert_eq!(
            read_server_message(&mut reader).unwrap_err(),
            ProtocolError::InvalidJson
        );
    }

    #[test]
    fn escaped_text_chunks_stay_below_the_jsonl_limit() {
        let value = "\0".repeat(MAX_TEXT_CHUNK_BYTES * 2 + 1);
        for (part, chunk) in utf8_chunks(&value).into_iter().enumerate() {
            let message = ServerMessage::TextChunk(TextChunkMessage {
                v: PROTOCOL_VERSION,
                seq: 1,
                request_id: REQUEST_ID.to_string(),
                stream: TextStream::Raw,
                part: part as u32,
                last: false,
                text: chunk.to_string(),
            });
            let mut output = Vec::new();
            write_server_message(&mut output, &message).unwrap();
            assert!(output.len() <= MAX_JSONL_BYTES);
        }
    }
}
