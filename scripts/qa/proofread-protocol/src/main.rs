#[path = "../../../../frontend/src-tauri/src/sensevoice_engine/model.rs"]
mod sensevoice_model;
#[path = "../../../../frontend/src-tauri/src/transcript_file_store.rs"]
mod transcript_file_store;
// JSON stdin/stdout adapter; includes the same source files as the desktop app.
#[path = "../../../../frontend/src-tauri/src/transcript_text_edit.rs"]
mod transcript_text_edit;
#[path = "../../../../frontend/src-tauri/src/proofread_protocol.rs"]
mod proofread_protocol;
use proofread_protocol::*;
use serde::Deserialize;
use std::io::Read;

#[derive(Deserialize)]
struct Request {
    rows: Vec<ProofreadRow>,
    #[serde(default)]
    context: String,
    #[serde(default)]
    start: usize,
    indices: Option<Vec<usize>>,
    raw: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let request: Request = serde_json::from_str(&input)?;
    let output = if let Some(indices) = request.indices {
        let batch: Vec<_> = indices.iter().map(|index| {
            request.rows.get(*index).map(|row| (*index, row)).ok_or("invalid index")
        }).collect::<Result<_, _>>()?;
        serde_json::json!({
            "prompt": build_retry_user_prompt(&batch, &request.context),
            "parsed": request.raw.map(|raw| parse_candidates(&raw, &batch, &request.context))
        })
    } else {
        let batches = build_review_batches(&request.rows, request.start);
        serde_json::json!({
            "version": PROMPT_VERSION, "system": SYSTEM_PROMPT,
            "batches": batches.iter().map(|batch| serde_json::json!({
                "indices": batch.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
                "prompt": build_batch_user_prompt(batch, &request.context)
            })).collect::<Vec<_>>()
        })
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}
