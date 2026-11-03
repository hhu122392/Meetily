//! Local SenseVoiceSmall inference using the sherpa-onnx export of the original
//! model. No Python, downloaded code, HTTP client or external service is used.
use anyhow::{bail, ensure, Context, Result};
use kaldi_fbank::{FbankOptions, OnlineFbank, WindowType};
use ndarray::{Array1, Array3};
use ort::{session::Session, value::TensorRef};
use std::{collections::HashMap, path::Path};

pub struct SenseVoiceModel {
    session: Session,
    tokens: Vec<String>,
    mean: Vec<f32>,
    scale: Vec<f32>,
    languages: HashMap<String, i32>,
    itn: i32,
    normalize: bool,
}

impl SenseVoiceModel {
    pub fn new(directory: &Path) -> Result<Self> {
        verify_artifacts(directory)?;
        crate::ensure_onnx_runtime_available()?;
        let session = Session::builder()?
            .with_intra_threads(std::thread::available_parallelism().map(|n| n.get().min(4)).unwrap_or(2))?
            .commit_from_file(directory.join("model.int8.onnx"))?;
        let metadata = session.metadata()?;
        let value = |key: &str| -> Result<String> {
            metadata.custom(key)?.with_context(|| format!("SenseVoice 缺少模型元数据：{key}"))
        };
        ensure!(value("lfr_window_size")? == "7" && value("lfr_window_shift")? == "6", "不支持的 SenseVoice 特征格式");
        let floats = |key: &str| -> Result<Vec<f32>> {
            value(key)?.split(',').map(|s| s.trim().parse().map_err(Into::into)).collect()
        };
        let mean = floats("neg_mean")?;
        let scale = floats("inv_stddev")?;
        ensure!(mean.len() == 560 && scale.len() == 560, "SenseVoice 归一化参数损坏");
        let languages = ["auto", "zh", "en", "yue", "ja", "ko"].iter()
            .map(|lang| Ok((lang.to_string(), value(&format!("lang_{lang}"))?.parse()?)))
            .collect::<Result<HashMap<_, _>>>()?;
        let itn = value("with_itn")?.parse()?;
        let normalize = value("normalize_samples")? == "1";
        let vocab_size: usize = value("vocab_size")?.parse()?;
        ensure!((1..100_000).contains(&vocab_size), "SenseVoice 词表大小无效");
        let mut tokens = vec![String::new(); vocab_size];
        for line in std::fs::read_to_string(directory.join("tokens.txt"))?.lines() {
            let (token, index) = line.rsplit_once(' ').context("SenseVoice 词表格式错误")?;
            let index: usize = index.parse()?;
            ensure!(index < vocab_size, "SenseVoice 词表索引越界");
            tokens[index] = token.to_owned();
        }
        ensure!(tokens.iter().all(|t| !t.is_empty()), "SenseVoice 词表不完整");
        drop(metadata);
        Ok(Self { session, tokens, mean, scale, languages, itn, normalize })
    }

    pub fn transcribe(&mut self, audio: &[f32], language: Option<&str>) -> Result<String> {
        // Existing recording/import pipelines segment before inference. Reject an
        // unbounded request instead of allocating hours of encoder activations.
        ensure!(audio.len() <= 30 * 16000, "SenseVoice 单段音频不得超过 30 秒，请先分段");
        ensure!(audio.iter().all(|v| v.is_finite()), "音频包含无效采样");
        if audio.len() < 400 { return Ok(String::new()); }
        let language = language.filter(|s| !s.is_empty()).unwrap_or("auto");
        let lang = *self.languages.get(language).with_context(|| format!("SenseVoice 不支持语言：{language}"))?;
        let mut options = FbankOptions::default();
        options.frame_opts.dither = 0.0;
        options.frame_opts.window_type = WindowType::Hamming;
        options.mel_opts.num_bins = 80;
        options.mel_opts.high_freq = 0.0;
        let mut fbank = OnlineFbank::new(options);
        let samples: Vec<f32> = audio.iter().map(|s| if self.normalize { *s } else { s * 32768.0 }).collect();
        fbank.accept_waveform(16000.0, &samples);
        fbank.input_finished();
        let frames = fbank.num_frames_ready() as usize;
        if frames == 0 { return Ok(String::new()); }
        let rows = frames.div_ceil(6);
        let mut features = Array3::<f32>::zeros((1, rows, 560));
        for row in 0..rows {
            for offset in 0..7 {
                let index = (row * 6 + offset).saturating_sub(3).min(frames - 1);
                let frame = fbank.get_frame(index as i32);
                for col in 0..80 {
                    let dim = offset * 80 + col;
                    features[[0, row, dim]] = (frame[col] + self.mean[dim]) * self.scale[dim];
                }
            }
        }
        let lengths = Array1::from_vec(vec![rows as i32]);
        let language = Array1::from_vec(vec![lang]);
        let itn = Array1::from_vec(vec![self.itn]);
        let outputs = self.session.run(ort::inputs![
            TensorRef::from_array_view(&features)?, TensorRef::from_array_view(&lengths)?,
            TensorRef::from_array_view(&language)?, TensorRef::from_array_view(&itn)?
        ])?;
        let (shape, logits) = outputs[0].try_extract_tensor::<f32>()?;
        ensure!(shape.len() == 3 && shape[0] == 1 && shape[2] as usize == self.tokens.len(), "SenseVoice 输出形状错误");
        let mut previous = usize::MAX;
        let mut ids = Vec::new();
        for frame in logits.chunks_exact(self.tokens.len()) {
            let id = frame.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(id, _)| id).unwrap_or(0);
            if id != previous && id != 0 { ids.push(id); }
            previous = id;
        }
        decode_tokens(&ids, &self.tokens)
    }
}

pub fn verify_artifacts(directory: &Path) -> Result<()> {
    for filename in ["model.int8.onnx", "tokens.txt"] { verify_artifact(&directory.join(filename))?; }
    Ok(())
}

pub fn verify_artifact(path: &Path) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let expected = match path.file_name().and_then(|s| s.to_str()) {
        Some("model.int8.onnx") => "c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51",
        Some("tokens.txt") => "f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc",
        _ => bail!("未知 SenseVoice 文件"),
    };
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 { break; }
        hash.update(&buffer[..n]);
    }
    ensure!(format!("{:x}", hash.finalize()) == expected, "SenseVoice 文件校验失败，请删除损坏模型后重新下载");
    Ok(())
}

fn decode_tokens(ids: &[usize], tokens: &[String]) -> Result<String> {
    let mut text = String::new();
    for &id in ids {
        let Some(token) = tokens.get(id) else { bail!("SenseVoice 输出词元越界"); };
        // Remove only reserved model tags, never arbitrary speech or repeated text.
        if token.starts_with("<|") && token.ends_with("|>") { continue; }
        if matches!(token.as_str(), "<blank>" | "<s>" | "</s>" | "<unk>") { continue; }
        text.push_str(&token.replace('▁', " "));
    }
    Ok(text.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tags_do_not_remove_speech_or_repetitions() {
        let tokens = ["<|zh|>", "<|NEUTRAL|>", "会议", "▁API", "好"].map(String::from);
        assert_eq!(decode_tokens(&[0, 1, 2, 3, 4, 4], &tokens).unwrap(), "会议 API好好");
        assert!(decode_tokens(&[99], &tokens).is_err());
    }

    #[test]
    fn modified_vocabulary_is_rejected_before_inference() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tokens.txt");
        std::fs::write(&file, "forged 0\n").unwrap();
        assert!(verify_artifact(&file).unwrap_err().to_string().contains("校验失败"));
    }

    #[test]
    #[ignore = "需要 SENSEVOICE_TEST_DIR 指向已校验的模型及五种语言公开 WAV 样本"]
    fn actual_model_transcribes_five_languages_and_rejects_invalid_input() {
        let dir = std::path::PathBuf::from(std::env::var("SENSEVOICE_TEST_DIR").unwrap());
        let mut model = SenseVoiceModel::new(&dir).unwrap();
        for language in ["zh", "en", "yue", "ja", "ko"] {
            let wav = crate::audio::decoder::decode_audio_file(&dir.join(format!("{language}.wav"))).unwrap();
            assert_eq!(wav.sample_rate, 16000);
            assert_eq!(wav.channels, 1);
            let audio = wav.to_whisper_format();
            let text = model.transcribe(&audio, Some(language)).unwrap();
            assert!(!text.is_empty(), "{language}");
            assert!(!text.contains("<|"));
            println!("{language}: {text}");
        }
        assert!(model.transcribe(&[], None).unwrap().is_empty());
        assert!(model.transcribe(&[f32::NAN; 400], None).is_err());
        assert!(model.transcribe(&vec![0.0; 30 * 16000 + 1], None).is_err());
        assert!(model.transcribe(&[0.0; 400], Some("fr")).is_err());
    }
}
