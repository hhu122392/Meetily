use std::io::{BufReader, BufWriter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use moss_helper::native::{self, NativeError};
use moss_helper::protocol::{
    is_uuid, read_client_message, utf8_chunks, write_server_message, AcceptedMessage,
    CancelledMessage, ClientMessage, CompletedMessage, CompletionStatus, ErrorCode, FailedMessage,
    HeartbeatMessage, HelloMessage, Operation, ProbeResultMessage, ProtocolError, SegmentMessage,
    ServerMessage, ShutdownMessage, StatusMessage, TaskPhase, TextChunkMessage, TextStream,
    PROTOCOL_VERSION,
};
use sha2::{Digest, Sha256};

const HELPER_VERSION: &str = env!("CARGO_PKG_VERSION");

struct Output<W> {
    writer: W,
    next_seq: u64,
}

const TASK_RUNNING: u8 = 0;
const TASK_CANCELLING: u8 = 1;
const TASK_SHUTTING_DOWN: u8 = 2;
const TASK_FAILING_PROTOCOL_UNEXPECTED: u8 = 3;
const TASK_COMPLETING: u8 = 4;
const TASK_FAILING_NATIVE: u8 = 5;
const TASK_FAILING_PROTOCOL_LINE_TOO_LONG: u8 = 6;
const TASK_FAILING_PROTOCOL_INVALID_JSON: u8 = 7;
const TASK_FAILING_PROTOCOL_UNSUPPORTED_VERSION: u8 = 8;
const TASK_FAILING_PROTOCOL_INVALID_REQUEST_ID: u8 = 9;
const TASK_FAILING_PROTOCOL_INVALID_CLIENT_SEQ: u8 = 10;
const TASK_FAILING_PROTOCOL_INVALID_CONTEXT_HASH: u8 = 11;
const TASK_FAILING_PROTOCOL_IO: u8 = 12;

impl<W: std::io::Write> Output<W> {
    fn new(writer: W) -> Self {
        Self {
            writer,
            next_seq: 1,
        }
    }

    fn seq(&mut self) -> u64 {
        let value = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        value
    }

    fn write(&mut self, message: &ServerMessage) -> bool {
        write_server_message(&mut self.writer, message).is_ok()
    }

    fn failed(
        &mut self,
        request_id: Option<String>,
        code: ErrorCode,
        phase: &'static str,
        native_status: Option<i32>,
        message: &'static str,
    ) -> bool {
        let seq = self.seq();
        self.write(&ServerMessage::Failed(FailedMessage {
            v: PROTOCOL_VERSION,
            seq,
            request_id,
            terminal: true,
            code,
            phase: phase.to_string(),
            retryable: false,
            native_status,
            message: message.to_string(),
        }))
    }
}

fn main() {
    let environment_error = native::harden_process_environment().err();
    let stdout = std::io::stdout();
    let mut output = Output::new(BufWriter::new(stdout));
    if !output.write(&ServerMessage::Hello(HelloMessage {
        v: PROTOCOL_VERSION,
        seq: 0,
        helper_version: HELPER_VERSION.to_string(),
        pid: std::process::id(),
        capabilities: ["probe", "transcribe", "cancel", "shutdown"]
            .into_iter()
            .map(str::to_string)
            .collect(),
    })) {
        std::process::exit(2);
    }
    if let Some(error) = environment_error {
        eprintln!("moss-helper: process DLL search hardening failed");
        output.failed(
            None,
            error.code,
            error.phase,
            error.native_status,
            error.message,
        );
        std::process::exit(2);
    }

    let mut reader = BufReader::new(std::io::stdin());
    let first = match read_client_message(&mut reader) {
        Ok(message) => message,
        Err(error) => {
            output.failed(
                None,
                error.code(),
                "protocol",
                None,
                "the first protocol message is invalid",
            );
            std::process::exit(2);
        }
    };
    if let Err(error) = first.validate(1) {
        let request_id = is_uuid(first.request_id()).then(|| first.request_id().to_string());
        output.failed(
            request_id,
            error.code(),
            "protocol",
            None,
            "the first protocol message failed validation",
        );
        std::process::exit(2);
    }

    let success = match first {
        ClientMessage::Probe(command) => handle_probe(&mut output, command),
        ClientMessage::Transcribe(command) => {
            let request_id = command.request_id.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            let terminal_state = Arc::new(std::sync::atomic::AtomicU8::new(TASK_RUNNING));
            start_control_reader(
                reader,
                request_id.clone(),
                cancel.clone(),
                terminal_state.clone(),
            );
            handle_transcribe(&mut output, command, cancel, &terminal_state)
        }
        ClientMessage::Cancel(command) => {
            let seq = output.seq();
            output.write(&ServerMessage::Cancelled(CancelledMessage {
                v: PROTOCOL_VERSION,
                seq,
                request_id: command.request_id,
                terminal: true,
                partial_result_available: false,
            }))
        }
        ClientMessage::Shutdown(command) => {
            let seq = output.seq();
            output.write(&ServerMessage::Shutdown(ShutdownMessage {
                v: PROTOCOL_VERSION,
                seq,
                request_id: command.request_id,
                terminal: true,
            }))
        }
    };
    if !success {
        std::process::exit(2);
    }
}

fn handle_probe<W: std::io::Write>(
    output: &mut Output<W>,
    command: moss_helper::protocol::ProbeCommand,
) -> bool {
    let request_id = command.request_id;
    let seq = output.seq();
    if !output.write(&ServerMessage::Accepted(AcceptedMessage {
        v: PROTOCOL_VERSION,
        seq,
        request_id: request_id.clone(),
        operation: Operation::Probe,
        context_sha256: None,
    })) {
        return false;
    }
    let runtime = command.runtime;
    let device = command.device;
    match run_native(move || native::probe_runtime(&runtime, &device)) {
        Ok(probe) => {
            let seq = output.seq();
            output.write(&ServerMessage::ProbeResult(ProbeResultMessage {
                v: PROTOCOL_VERSION,
                seq,
                request_id,
                terminal: true,
                runtime_version: probe.runtime_version,
                runtime_commit: probe.runtime_commit,
                abi_verified: true,
                backend: probe.backend,
                device: probe.device,
            }))
        }
        Err(error) => write_native_error(output, request_id, error),
    }
}

fn handle_transcribe<W: std::io::Write>(
    output: &mut Output<W>,
    command: moss_helper::protocol::TranscribeCommand,
    cancel: Arc<AtomicBool>,
    terminal_state: &std::sync::atomic::AtomicU8,
) -> bool {
    let task_started = std::time::Instant::now();
    let request_id = command.request_id.clone();
    let context_sha256 = command.context_sha256.clone();
    let seq = output.seq();
    if !output.write(&ServerMessage::Accepted(AcceptedMessage {
        v: PROTOCOL_VERSION,
        seq,
        request_id: request_id.clone(),
        operation: Operation::Transcribe,
        context_sha256: Some(context_sha256.clone()),
    })) {
        return false;
    }

    if !write_phase(
        output,
        &request_id,
        &context_sha256,
        TaskPhase::Preflight,
        task_started,
    ) {
        return false;
    }
    let native_command = command.clone();
    let native_cancel = cancel.clone();
    match run_native_with_heartbeats(
        output,
        &request_id,
        &context_sha256,
        task_started,
        cancel.clone(),
        move || native::transcribe(&native_command, &native_cancel),
    ) {
        Ok(result) => {
            if terminal_state
                .compare_exchange(
                    TASK_RUNNING,
                    TASK_COMPLETING,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_err()
            {
                return write_control_terminal(
                    output,
                    request_id,
                    terminal_state.load(Ordering::SeqCst),
                );
            }
            if !write_phase(
                output,
                &request_id,
                &context_sha256,
                TaskPhase::ResultStreaming,
                task_started,
            ) || !emit_text(output, &request_id, TextStream::Raw, &result.raw_text)
                || !emit_text(output, &request_id, TextStream::Clean, &result.clean_text)
            {
                return false;
            }
            let mut last_timestamp_ms = 0;
            for (index, segment) in result.segments.iter().enumerate() {
                last_timestamp_ms = last_timestamp_ms.max(segment.t1_ms);
                let chunks = utf8_chunks(&segment.text);
                let last_part = chunks.len().saturating_sub(1);
                for (part, chunk) in chunks.into_iter().enumerate() {
                    let seq = output.seq();
                    if !output.write(&ServerMessage::Segment(SegmentMessage {
                        v: PROTOCOL_VERSION,
                        seq,
                        request_id: request_id.clone(),
                        segment_index: index.try_into().unwrap_or(u32::MAX),
                        t0_ms: segment.t0_ms,
                        t1_ms: segment.t1_ms,
                        speaker_id: segment.speaker_id,
                        text_part: part.try_into().unwrap_or(u32::MAX),
                        text_last: part == last_part,
                        text: chunk.to_string(),
                    })) {
                        return false;
                    }
                }
            }
            let seq = output.seq();
            let wall_elapsed_ms = elapsed_ms(task_started);
            let duration_seconds = command.audio.samples as f64 / 16_000.0;
            output.write(&ServerMessage::Completed(CompletedMessage {
                v: PROTOCOL_VERSION,
                seq,
                request_id,
                context_sha256,
                terminal: true,
                status: CompletionStatus::Ok,
                backend: result.probe.backend,
                device_description: result.probe.device.description,
                native_run_elapsed_ms: result.wall_elapsed_ms,
                native_rtf: result.rtf,
                wall_elapsed_ms,
                wall_rtf: wall_elapsed_ms as f64 / 1000.0 / duration_seconds,
                last_timestamp_ms,
                segment_count: result.segments.len().try_into().unwrap_or(u32::MAX),
                raw_text_sha256: sha256_text(&result.raw_text),
                clean_text_sha256: sha256_text(&result.clean_text),
                was_aborted: result.was_aborted,
                was_truncated: result.was_truncated,
                native_session_limits: result.session_limits,
                native_timings: result.timings,
                language_requested: result.language_requested,
                language_resolved: result.language_resolved,
                decode_parameters_json: result.decode_parameters_json,
                decode_parameters_sha256: result.decode_parameters_sha256,
            }))
        }
        Err(error) if error.code == ErrorCode::Cancelled => {
            let _ = terminal_state.compare_exchange(
                TASK_RUNNING,
                TASK_CANCELLING,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            write_control_terminal(output, request_id, terminal_state.load(Ordering::SeqCst))
        }
        Err(error) => {
            // A fast preflight failure can race an already-closed stdin control
            // pipe. Give the control reader one short scheduling window so EOF
            // is deterministically reported as cancellation, never as a native
            // failure selected only by thread timing.
            let decision_deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(25);
            while terminal_state.load(Ordering::SeqCst) == TASK_RUNNING
                && !cancel.load(Ordering::SeqCst)
                && std::time::Instant::now() < decision_deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            match terminal_state.compare_exchange(
                TASK_RUNNING,
                TASK_FAILING_NATIVE,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => write_native_error(output, request_id, error),
                Err(state) => write_control_terminal(output, request_id, state),
            }
        }
    }
}

fn write_control_terminal<W: std::io::Write>(
    output: &mut Output<W>,
    request_id: String,
    state: u8,
) -> bool {
    match state {
        TASK_CANCELLING => {
            let seq = output.seq();
            output.write(&ServerMessage::Cancelled(CancelledMessage {
                v: PROTOCOL_VERSION,
                seq,
                request_id,
                terminal: true,
                partial_result_available: false,
            }))
        }
        TASK_SHUTTING_DOWN => {
            let seq = output.seq();
            output.write(&ServerMessage::Shutdown(ShutdownMessage {
                v: PROTOCOL_VERSION,
                seq,
                request_id,
                terminal: true,
            }))
        }
        state if protocol_failure_code(state).is_some() => {
            output.failed(
                Some(request_id),
                protocol_failure_code(state).unwrap_or(ErrorCode::ProtocolUnexpectedMessage),
                "control_protocol",
                None,
                "a control message failed validation",
            );
            false
        }
        _ => false,
    }
}

fn protocol_failure_state(error: &ProtocolError) -> u8 {
    match error {
        ProtocolError::LineTooLong => TASK_FAILING_PROTOCOL_LINE_TOO_LONG,
        ProtocolError::InvalidJson => TASK_FAILING_PROTOCOL_INVALID_JSON,
        ProtocolError::UnsupportedVersion => TASK_FAILING_PROTOCOL_UNSUPPORTED_VERSION,
        ProtocolError::InvalidRequestId => TASK_FAILING_PROTOCOL_INVALID_REQUEST_ID,
        ProtocolError::InvalidClientSeq => TASK_FAILING_PROTOCOL_INVALID_CLIENT_SEQ,
        ProtocolError::InvalidContextHash => TASK_FAILING_PROTOCOL_INVALID_CONTEXT_HASH,
        ProtocolError::Io => TASK_FAILING_PROTOCOL_IO,
        ProtocolError::Eof => TASK_CANCELLING,
    }
}

fn protocol_failure_code(state: u8) -> Option<ErrorCode> {
    match state {
        TASK_FAILING_PROTOCOL_UNEXPECTED => Some(ErrorCode::ProtocolUnexpectedMessage),
        TASK_FAILING_PROTOCOL_LINE_TOO_LONG => Some(ErrorCode::ProtocolLineTooLong),
        TASK_FAILING_PROTOCOL_INVALID_JSON => Some(ErrorCode::ProtocolInvalidJson),
        TASK_FAILING_PROTOCOL_UNSUPPORTED_VERSION => Some(ErrorCode::ProtocolUnsupportedVersion),
        TASK_FAILING_PROTOCOL_INVALID_REQUEST_ID => Some(ErrorCode::ProtocolInvalidRequestId),
        TASK_FAILING_PROTOCOL_INVALID_CLIENT_SEQ => Some(ErrorCode::ProtocolInvalidClientSeq),
        TASK_FAILING_PROTOCOL_INVALID_CONTEXT_HASH => Some(ErrorCode::ProtocolInvalidContextHash),
        TASK_FAILING_PROTOCOL_IO => Some(ErrorCode::ProtocolIo),
        _ => None,
    }
}

fn write_phase<W: std::io::Write>(
    output: &mut Output<W>,
    request_id: &str,
    context_sha256: &str,
    phase: TaskPhase,
    started: std::time::Instant,
) -> bool {
    let seq = output.seq();
    output.write(&ServerMessage::Status(StatusMessage {
        v: PROTOCOL_VERSION,
        seq,
        request_id: request_id.to_string(),
        context_sha256: context_sha256.to_string(),
        phase,
        elapsed_ms: elapsed_ms(started),
    }))
}

fn emit_text<W: std::io::Write>(
    output: &mut Output<W>,
    request_id: &str,
    stream: TextStream,
    text: &str,
) -> bool {
    let chunks = utf8_chunks(text);
    let last_index = chunks.len().saturating_sub(1);
    for (index, chunk) in chunks.into_iter().enumerate() {
        let seq = output.seq();
        if !output.write(&ServerMessage::TextChunk(TextChunkMessage {
            v: PROTOCOL_VERSION,
            seq,
            request_id: request_id.to_string(),
            stream,
            part: index.try_into().unwrap_or(u32::MAX),
            last: index == last_index,
            text: chunk.to_string(),
        })) {
            return false;
        }
    }
    true
}

fn write_native_error<W: std::io::Write>(
    output: &mut Output<W>,
    request_id: String,
    error: NativeError,
) -> bool {
    eprintln!(
        "moss-helper: phase={} code={:?} native_status={:?}",
        error.phase, error.code, error.native_status
    );
    output.failed(
        Some(request_id),
        error.code,
        error.phase,
        error.native_status,
        error.message,
    );
    false
}

fn start_control_reader(
    mut reader: BufReader<std::io::Stdin>,
    request_id: String,
    cancel: Arc<AtomicBool>,
    terminal_state: Arc<std::sync::atomic::AtomicU8>,
) {
    std::thread::spawn(move || {
        let expected_seq = 2;
        let message = match read_client_message(&mut reader) {
            Ok(message) => message,
            Err(error) => {
                let target = protocol_failure_state(&error);
                let _ = terminal_state.compare_exchange(
                    TASK_RUNNING,
                    target,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                cancel.store(true, Ordering::SeqCst);
                return;
            }
        };
        if let Err(error) = message.validate(expected_seq) {
            let _ = terminal_state.compare_exchange(
                TASK_RUNNING,
                protocol_failure_state(&error),
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            cancel.store(true, Ordering::SeqCst);
            return;
        }
        if message.request_id() != request_id {
            let _ = terminal_state.compare_exchange(
                TASK_RUNNING,
                TASK_FAILING_PROTOCOL_INVALID_REQUEST_ID,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            cancel.store(true, Ordering::SeqCst);
            return;
        }
        match message {
            ClientMessage::Cancel(_) => {
                let _ = terminal_state.compare_exchange(
                    TASK_RUNNING,
                    TASK_CANCELLING,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                cancel.store(true, Ordering::SeqCst);
            }
            ClientMessage::Shutdown(_) => {
                let _ = terminal_state.compare_exchange(
                    TASK_RUNNING,
                    TASK_SHUTTING_DOWN,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                cancel.store(true, Ordering::SeqCst);
            }
            ClientMessage::Probe(_) | ClientMessage::Transcribe(_) => {
                let _ = terminal_state.compare_exchange(
                    TASK_RUNNING,
                    TASK_FAILING_PROTOCOL_UNEXPECTED,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                cancel.store(true, Ordering::SeqCst);
            }
        }
    });
}

fn sha256_text(value: &str) -> String {
    format!("{:X}", Sha256::digest(value.as_bytes()))
}

fn elapsed_ms(started: std::time::Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn run_native_with_heartbeats<T, F, W>(
    output: &mut Output<W>,
    request_id: &str,
    context_sha256: &str,
    started: std::time::Instant,
    cancel: Arc<AtomicBool>,
    operation: F,
) -> Result<T, NativeError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, NativeError> + Send + 'static,
    W: std::io::Write,
{
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("moss-native".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let _ = sender.send(operation());
        })
        .map_err(|_| NativeError {
            code: ErrorCode::Internal,
            phase: "native_thread",
            message: "the native worker thread could not be created",
            native_status: None,
        })?;

    if !write_phase(
        output,
        request_id,
        context_sha256,
        TaskPhase::NativeRunning,
        started,
    ) {
        cancel.store(true, Ordering::SeqCst);
        return Err(NativeError {
            code: ErrorCode::ProtocolIo,
            phase: "stdout",
            message: "the protocol output pipe is unavailable",
            native_status: None,
        });
    }

    loop {
        match receiver.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(result) => return result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let seq = output.seq();
                if !output.write(&ServerMessage::Heartbeat(HeartbeatMessage {
                    v: PROTOCOL_VERSION,
                    seq,
                    request_id: request_id.to_string(),
                    context_sha256: context_sha256.to_string(),
                    phase: TaskPhase::NativeRunning,
                    elapsed_ms: elapsed_ms(started),
                })) {
                    cancel.store(true, Ordering::SeqCst);
                    return Err(NativeError {
                        code: ErrorCode::ProtocolIo,
                        phase: "stdout",
                        message: "the protocol output pipe is unavailable",
                        native_status: None,
                    });
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(NativeError {
                    code: ErrorCode::Internal,
                    phase: "native_thread",
                    message: "the native worker thread terminated unexpectedly",
                    native_status: None,
                });
            }
        }
    }
}

fn run_native<T, F>(operation: F) -> Result<T, NativeError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, NativeError> + Send + 'static,
{
    let thread = std::thread::Builder::new()
        .name("moss-native".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(operation)
        .map_err(|_| NativeError {
            code: ErrorCode::Internal,
            phase: "native_thread",
            message: "the native worker thread could not be created",
            native_status: None,
        })?;
    thread.join().map_err(|_| NativeError {
        code: ErrorCode::Internal,
        phase: "native_thread",
        message: "the native worker thread terminated unexpectedly",
        native_status: None,
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_protocol_failures_keep_their_stable_error_codes() {
        for error in [
            ProtocolError::LineTooLong,
            ProtocolError::InvalidJson,
            ProtocolError::UnsupportedVersion,
            ProtocolError::InvalidRequestId,
            ProtocolError::InvalidClientSeq,
            ProtocolError::InvalidContextHash,
            ProtocolError::Io,
        ] {
            assert_eq!(
                protocol_failure_code(protocol_failure_state(&error)),
                Some(error.code())
            );
        }
        assert_eq!(protocol_failure_state(&ProtocolError::Eof), TASK_CANCELLING);
        assert_eq!(
            protocol_failure_code(TASK_FAILING_PROTOCOL_UNEXPECTED),
            Some(ErrorCode::ProtocolUnexpectedMessage)
        );
    }
}
