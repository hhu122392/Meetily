use std::io::{Read, Write};
use std::process::{Command, Output, Stdio};

use moss_helper::protocol::{MAX_JSONL_BYTES, PROTOCOL_VERSION};
use serde_json::Value;

const REQUEST_ID: &str = "798d8c63-5ff1-40e3-9db8-0f706aeb930a";

fn run_helper(input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_moss-helper"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn helper");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input)
        .expect("write helper input");
    child.wait_with_output().expect("wait for helper")
}

fn run_helper_with_stdin_held_open(input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_moss-helper"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn helper");
    let mut stdin = child.stdin.take().expect("piped stdin");
    stdin.write_all(input).expect("write helper input");
    stdin.flush().expect("flush helper input");
    let status = child.wait().expect("wait for helper");
    drop(stdin);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .unwrap();
    Output {
        status,
        stdout,
        stderr,
    }
}

fn json_lines(output: &[u8]) -> Vec<Value> {
    String::from_utf8(output.to_vec())
        .expect("stdout UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON value per line"))
        .collect()
}

#[test]
fn shutdown_is_a_single_task_jsonl_exchange() {
    let input = format!(
        "{{\"type\":\"shutdown\",\"v\":{PROTOCOL_VERSION},\"request_id\":\"{REQUEST_ID}\",\"client_seq\":1}}\n"
    );
    let output = run_helper(input.as_bytes());
    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let messages = json_lines(&output.stdout);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["type"], "hello");
    assert_eq!(messages[0]["seq"], 0);
    assert_eq!(messages[1]["type"], "shutdown");
    assert_eq!(messages[1]["seq"], 1);
    assert_eq!(messages[1]["request_id"], REQUEST_ID);
    assert_eq!(messages[1]["terminal"], true);
}

#[test]
fn transcribe_stdin_eof_is_a_single_cancelled_terminal() {
    let input = format!(
        concat!(
            "{{\"type\":\"transcribe\",\"v\":{},\"request_id\":\"{}\",",
            "\"client_seq\":1,\"context_sha256\":\"{}\",",
            "\"runtime\":{{\"directory\":\"D:\\\\private\\\\runtime\"}},",
            "\"device\":{{\"kind\":\"vulkan\",\"description\":",
            "\"Intel(R) Arc(TM) Graphics\",\"device_id\":null,",
            "\"allow_primary_fallback\":false}},",
            "\"model\":{{\"path\":\"D:\\\\private\\\\model.gguf\",",
            "\"bytes\":1,\"sha256\":\"{}\"}},",
            "\"audio\":{{\"path\":\"D:\\\\private\\\\audio.f32le\",",
            "\"format\":\"f32le\",\"sample_rate_hz\":16000,\"channels\":1,",
            "\"samples\":1,\"bytes\":4,\"sha256\":\"{}\"}},",
            "\"language_requested\":\"zh-CN\",",
            "\"decode_parameters_json\":\"{{\\\"language\\\":\\\"zh\\\",\\\"timestamps\\\":\\\"segment\\\",\\\"diarize\\\":\\\"on\\\"}}\",",
            "\"decode_parameters_sha256\":\"{}\"}}\n"
        ),
        PROTOCOL_VERSION,
        REQUEST_ID,
        "A".repeat(64),
        "0".repeat(64),
        "0".repeat(64),
        moss_helper::native::moss_decode_parameters_sha256(),
    );
    let output = run_helper(input.as_bytes());
    assert!(output.status.success());
    let messages = json_lines(&output.stdout);
    assert_eq!(messages.last().unwrap()["type"], "cancelled");
    assert_eq!(
        messages
            .iter()
            .filter(|value| value["terminal"] == true)
            .count(),
        1
    );
}

#[test]
fn cancel_is_terminal_without_starting_native_work() {
    let input = format!(
        "{{\"type\":\"cancel\",\"v\":{PROTOCOL_VERSION},\"request_id\":\"{REQUEST_ID}\",\"client_seq\":1,\"reason\":\"user\"}}\n"
    );
    let output = run_helper(input.as_bytes());
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let messages = json_lines(&output.stdout);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1]["type"], "cancelled");
    assert_eq!(messages[1]["request_id"], REQUEST_ID);
    assert_eq!(messages[1]["terminal"], true);
}

#[test]
fn transcribe_contract_failure_redacts_local_paths() {
    let input = format!(
        concat!(
            "{{\"type\":\"transcribe\",\"v\":{},\"request_id\":\"{}\",",
            "\"client_seq\":1,\"context_sha256\":\"{}\",",
            "\"runtime\":{{\"directory\":\"D:\\\\private\\\\runtime\"}},",
            "\"device\":{{\"kind\":\"vulkan\",\"description\":",
            "\"Intel(R) Arc(TM) Graphics\",\"device_id\":null,",
            "\"allow_primary_fallback\":false}},",
            "\"model\":{{\"path\":\"D:\\\\private\\\\model.gguf\",",
            "\"bytes\":1,\"sha256\":\"{}\"}},",
            "\"audio\":{{\"path\":\"D:\\\\private\\\\audio.f32le\",",
            "\"format\":\"f32le\",\"sample_rate_hz\":16000,\"channels\":1,",
            "\"samples\":1,\"bytes\":4,\"sha256\":\"{}\"}},",
            "\"language_requested\":\"zh-CN\",",
            "\"decode_parameters_json\":\"{{\\\"language\\\":\\\"zh\\\",\\\"timestamps\\\":\\\"segment\\\",\\\"diarize\\\":\\\"on\\\"}}\",",
            "\"decode_parameters_sha256\":\"{}\"}}\n"
        ),
        PROTOCOL_VERSION,
        REQUEST_ID,
        "A".repeat(64),
        "0".repeat(64),
        "0".repeat(64),
        moss_helper::native::moss_decode_parameters_sha256(),
    );
    let output = run_helper_with_stdin_held_open(input.as_bytes());
    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8(output.stdout).expect("stdout UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr UTF-8");
    assert!(!stdout.contains("private"));
    assert!(!stderr.contains("private"));
    let messages = json_lines(stdout.as_bytes());
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[1]["type"], "accepted");
    assert_eq!(messages[2]["type"], "status");
    assert_eq!(messages[2]["phase"], "preflight");
    assert_eq!(messages[3]["type"], "status");
    assert_eq!(messages[3]["phase"], "native_running");
    assert_eq!(messages[4]["type"], "failed");
    assert_eq!(messages[4]["code"], "MODEL_CONTRACT_MISMATCH");
    assert_eq!(messages[4]["request_id"], REQUEST_ID);
}

#[test]
fn unknown_fields_are_rejected_without_echoing_input() {
    let secret = r"C:\\private\\meeting.raw";
    let input = format!(
        "{{\"type\":\"shutdown\",\"v\":{PROTOCOL_VERSION},\"request_id\":\"{REQUEST_ID}\",\"client_seq\":1,\"secret_path\":\"{secret}\"}}\n"
    );
    let output = run_helper(input.as_bytes());
    assert_eq!(output.status.code(), Some(2));

    let stdout = String::from_utf8(output.stdout).expect("stdout UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("stderr UTF-8");
    assert!(!stdout.contains("private"));
    assert!(!stderr.contains("private"));
    let messages = json_lines(stdout.as_bytes());
    assert_eq!(messages[1]["code"], "PROTOCOL_INVALID_JSON");
    assert_eq!(messages[1]["request_id"], Value::Null);
}

#[test]
fn oversized_input_is_bounded_and_rejected() {
    let mut input = vec![b'x'; MAX_JSONL_BYTES + 1];
    input.push(b'\n');
    let output = run_helper(&input);
    assert_eq!(output.status.code(), Some(2));
    let messages = json_lines(&output.stdout);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1]["code"], "PROTOCOL_LINE_TOO_LONG");
    assert_eq!(messages[1]["terminal"], true);
}

#[cfg(windows)]
#[test]
#[ignore = "requires MOSS_TEST_RUNTIME_DIR and a real Intel Arc Vulkan runtime"]
fn application_directory_cwd_and_path_dll_decoys_cannot_hijack_runtime_probe() {
    let runtime = std::env::var_os("MOSS_TEST_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .expect("MOSS_TEST_RUNTIME_DIR is required for the ignored real-runtime gate");
    let temporary = tempfile::tempdir().unwrap();
    let attack_directory = temporary.path().join("中文 空格 假DLL");
    std::fs::create_dir(&attack_directory).unwrap();
    let copied_helper = attack_directory.join("moss-helper test.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_moss-helper"), &copied_helper).unwrap();
    for name in ["vulkan-1.dll", "MSVCP140.dll"] {
        std::fs::copy(runtime.join("transcribe.dll"), attack_directory.join(name)).unwrap();
    }

    let input = serde_json::json!({
        "type": "probe",
        "v": PROTOCOL_VERSION,
        "request_id": REQUEST_ID,
        "client_seq": 1,
        "runtime": { "directory": runtime },
        "device": {
            "kind": "vulkan",
            "description": "Intel(R) Arc(TM) Graphics",
            "device_id": null,
            "allow_primary_fallback": false
        }
    });
    let mut child = Command::new(&copied_helper)
        .current_dir(&attack_directory)
        .env("PATH", &attack_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "{input}").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let messages = json_lines(&output.stdout);
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[2]["type"], "probe_result");
    assert_eq!(messages[2]["backend"], "vulkan");
    assert_eq!(
        messages[2]["device"]["description"],
        "Intel(R) Arc(TM) Graphics"
    );
}

#[test]
fn native_environment_overrides_fail_before_protocol_start_without_echoing_values() {
    for name in [
        "VK_ADD_LAYER_PATH",
        "VK_DRIVER_FILES",
        "VK_ICD_FILENAMES",
        "VK_INSTANCE_LAYERS",
        "VK_LAYER_PATH",
        "VK_TEST_OVERRIDE",
        "VK_LOADER_DEBUG",
        "VULKAN_SDK",
        "TRANSCRIBE_TEST_OVERRIDE",
        "GGML_TEST_OVERRIDE",
        "GGML_BACKEND_PATH",
    ] {
        let marker = format!("private-{name}-value");
        let output = Command::new(env!("CARGO_BIN_EXE_moss-helper"))
            .env(name, &marker)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{name}");
        let messages = json_lines(&output.stdout);
        assert_eq!(messages.len(), 2, "{name}");
        assert_eq!(messages[0]["type"], "hello", "{name}");
        assert_eq!(messages[1]["type"], "failed", "{name}");
        assert_eq!(
            messages[1]["code"], "RUNTIME_ENVIRONMENT_FORBIDDEN",
            "{name}"
        );
        assert_eq!(messages[1]["phase"], "runtime_environment", "{name}");
        assert_eq!(messages[1]["terminal"], true, "{name}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(
            stderr, "moss-helper: process DLL search hardening failed\n",
            "{name}"
        );
        assert!(!stderr.contains(&marker), "{name}");
    }
}

#[test]
fn empty_native_environment_overrides_are_removed_without_failing() {
    for name in [
        "VK_TEST_OVERRIDE",
        "VK_LOADER_DEBUG",
        "TRANSCRIBE_EMPTY",
        "GGML_EMPTY",
    ] {
        let input = serde_json::json!({
            "type": "shutdown",
            "v": PROTOCOL_VERSION,
            "request_id": REQUEST_ID,
            "client_seq": 1
        });
        let mut child = Command::new(env!("CARGO_BIN_EXE_moss-helper"))
            .env(name, "")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(child.stdin.take().unwrap(), "{input}").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{name}");
        assert!(output.stderr.is_empty(), "{name}");
        let messages = json_lines(&output.stdout);
        assert_eq!(messages.len(), 2, "{name}");
        assert_eq!(messages[0]["type"], "hello", "{name}");
        assert_eq!(messages[1]["type"], "shutdown", "{name}");
    }
}
