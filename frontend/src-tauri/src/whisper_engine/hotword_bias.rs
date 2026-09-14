use std::collections::{BTreeMap, BTreeSet};
use std::os::raw::{c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use whisper_rs::{
    FullParams, WhisperContext, WhisperSysContext, WhisperSysState, WhisperTokenData,
};

pub const R10_HOTWORD_BIAS_SCHEMA_VERSION: u8 = 1;
pub const R10_HOTWORD_BIAS_STRATEGY: &str = "TOKEN_SEQUENCE_SUFFIX_MAX_LOGIT_BIAS";
pub const R10_HOTWORD_TOKENIZATION_VARIANTS: [&str; 2] = ["canonical", "leading_space"];
pub const R10_MAX_CANONICAL_TERMS: usize = 64;
pub const R10_MAX_TERM_CHARS: usize = 128;
pub const R10_MAX_TOKENS_PER_SEQUENCE: usize = 16;
const R10_TOKENIZE_CAPACITY: usize = 64;
const MAX_CALLBACK_HISTORY_TOKENS: usize = 4_096;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct WhisperHotwordBiasConfig {
    pub start_token_logit_add: f32,
    pub continuation_token_logit_add: f32,
    pub completion_token_logit_add: f32,
}

impl WhisperHotwordBiasConfig {
    pub const fn r10_frozen() -> Self {
        Self {
            start_token_logit_add: 1.5,
            continuation_token_logit_add: 3.0,
            completion_token_logit_add: 4.0,
        }
    }

    fn is_r10_frozen(self) -> bool {
        self == Self::r10_frozen()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WhisperHotwordTermDiagnostics {
    pub canonical_index: usize,
    pub term_sha256: String,
    pub token_sequence_count: usize,
    pub token_counts: Vec<usize>,
    pub token_sequence_sha256: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WhisperHotwordBiasDiagnostics {
    pub schema_version: u8,
    pub strategy: String,
    pub binding_sha256: String,
    pub canonical_terms_sha256: String,
    pub canonical_term_count: usize,
    pub token_sequence_set_sha256: String,
    pub token_sequence_count: usize,
    pub tokenization_variants: Vec<String>,
    pub maximum_tokens_per_sequence: usize,
    pub start_token_logit_add: f32,
    pub continuation_token_logit_add: f32,
    pub completion_token_logit_add: f32,
    pub duplicate_adjustment_rule: String,
    pub canonical_term_plaintext_stored: bool,
    pub raw_token_ids_stored: bool,
    pub terms: Vec<WhisperHotwordTermDiagnostics>,
    pub decode_call_count: u32,
    pub callback_invocations: u64,
    pub callbacks_with_adjustments: u64,
    pub logit_adjustment_count: u64,
    pub maximum_adjustments_in_one_callback: u64,
}

impl WhisperHotwordBiasDiagnostics {
    pub fn merge_decode(&mut self, other: &Self) -> Result<()> {
        if self.binding_sha256 != other.binding_sha256 {
            return Err(anyhow!(
                "hotword bias diagnostics binding changed between chunks"
            ));
        }
        self.decode_call_count = self
            .decode_call_count
            .checked_add(other.decode_call_count)
            .ok_or_else(|| anyhow!("hotword bias decode count overflow"))?;
        self.callback_invocations = self
            .callback_invocations
            .checked_add(other.callback_invocations)
            .ok_or_else(|| anyhow!("hotword bias callback count overflow"))?;
        self.callbacks_with_adjustments = self
            .callbacks_with_adjustments
            .checked_add(other.callbacks_with_adjustments)
            .ok_or_else(|| anyhow!("hotword bias adjusted callback count overflow"))?;
        self.logit_adjustment_count = self
            .logit_adjustment_count
            .checked_add(other.logit_adjustment_count)
            .ok_or_else(|| anyhow!("hotword bias adjustment count overflow"))?;
        self.maximum_adjustments_in_one_callback = self
            .maximum_adjustments_in_one_callback
            .max(other.maximum_adjustments_in_one_callback);
        Ok(())
    }
}

pub(crate) fn validate_hotword_bias_diagnostics(
    diagnostics: &WhisperHotwordBiasDiagnostics,
) -> bool {
    let config = WhisperHotwordBiasConfig {
        start_token_logit_add: diagnostics.start_token_logit_add,
        continuation_token_logit_add: diagnostics.continuation_token_logit_add,
        completion_token_logit_add: diagnostics.completion_token_logit_add,
    };
    if diagnostics.schema_version != R10_HOTWORD_BIAS_SCHEMA_VERSION
        || diagnostics.strategy != R10_HOTWORD_BIAS_STRATEGY
        || !is_sha256(&diagnostics.binding_sha256)
        || !is_sha256(&diagnostics.canonical_terms_sha256)
        || diagnostics.canonical_term_count == 0
        || diagnostics.canonical_term_count > R10_MAX_CANONICAL_TERMS
        || !is_sha256(&diagnostics.token_sequence_set_sha256)
        || diagnostics.token_sequence_count == 0
        || diagnostics.tokenization_variants != R10_HOTWORD_TOKENIZATION_VARIANTS.map(str::to_owned)
        || diagnostics.maximum_tokens_per_sequence != R10_MAX_TOKENS_PER_SEQUENCE
        || !config.is_r10_frozen()
        || diagnostics.duplicate_adjustment_rule != "maximum_not_sum"
        || diagnostics.canonical_term_plaintext_stored
        || diagnostics.raw_token_ids_stored
        || diagnostics.terms.len() != diagnostics.canonical_term_count
        || diagnostics.decode_call_count == 0
        || diagnostics.callback_invocations == 0
        || diagnostics.callbacks_with_adjustments > diagnostics.callback_invocations
        || diagnostics.logit_adjustment_count < diagnostics.callbacks_with_adjustments
        || diagnostics.maximum_adjustments_in_one_callback > diagnostics.logit_adjustment_count
        || (diagnostics.callbacks_with_adjustments == 0
            && (diagnostics.logit_adjustment_count != 0
                || diagnostics.maximum_adjustments_in_one_callback != 0))
        || (diagnostics.callbacks_with_adjustments > 0
            && diagnostics.maximum_adjustments_in_one_callback == 0)
    {
        return false;
    }
    for (index, term) in diagnostics.terms.iter().enumerate() {
        if term.canonical_index != index
            || !is_sha256(&term.term_sha256)
            || term.token_sequence_count == 0
            || term.token_counts.len() != term.token_sequence_count
            || term.token_sequence_sha256.len() != term.token_sequence_count
            || term
                .token_counts
                .iter()
                .any(|count| *count == 0 || *count > R10_MAX_TOKENS_PER_SEQUENCE)
            || term
                .token_sequence_sha256
                .iter()
                .any(|hash| !is_sha256(hash))
        {
            return false;
        }
    }
    hotword_bias_binding_sha256(diagnostics).ok().as_deref() == Some(&diagnostics.binding_sha256)
}

pub(crate) fn hotword_bias_binding_sha256(
    diagnostics: &WhisperHotwordBiasDiagnostics,
) -> Result<String> {
    let config = WhisperHotwordBiasConfig {
        start_token_logit_add: diagnostics.start_token_logit_add,
        continuation_token_logit_add: diagnostics.continuation_token_logit_add,
        completion_token_logit_add: diagnostics.completion_token_logit_add,
    };
    let binding = WhisperHotwordBiasBinding {
        schema_version: diagnostics.schema_version,
        strategy: &diagnostics.strategy,
        canonical_terms_sha256: &diagnostics.canonical_terms_sha256,
        canonical_term_count: diagnostics.canonical_term_count,
        token_sequence_set_sha256: &diagnostics.token_sequence_set_sha256,
        token_sequence_count: diagnostics.token_sequence_count,
        tokenization_variants: &R10_HOTWORD_TOKENIZATION_VARIANTS,
        maximum_tokens_per_sequence: diagnostics.maximum_tokens_per_sequence,
        config,
        duplicate_adjustment_rule: &diagnostics.duplicate_adjustment_rule,
        terms: &diagnostics.terms,
    };
    sha256_json(&binding)
}

#[derive(Serialize)]
struct WhisperHotwordBiasBinding<'a> {
    schema_version: u8,
    strategy: &'a str,
    canonical_terms_sha256: &'a str,
    canonical_term_count: usize,
    token_sequence_set_sha256: &'a str,
    token_sequence_count: usize,
    tokenization_variants: &'a [&'a str],
    maximum_tokens_per_sequence: usize,
    config: WhisperHotwordBiasConfig,
    duplicate_adjustment_rule: &'a str,
    terms: &'a [WhisperHotwordTermDiagnostics],
}

pub(crate) struct WhisperHotwordBiasState {
    n_vocab: usize,
    sequences: Vec<Vec<i32>>,
    config: WhisperHotwordBiasConfig,
    base_diagnostics: WhisperHotwordBiasDiagnostics,
    callback_invocations: AtomicU64,
    callbacks_with_adjustments: AtomicU64,
    logit_adjustment_count: AtomicU64,
    maximum_adjustments_in_one_callback: AtomicU64,
}

impl WhisperHotwordBiasState {
    pub(crate) fn new(
        context: &WhisperContext,
        canonical_terms: &[String],
        config: WhisperHotwordBiasConfig,
    ) -> Result<Self> {
        if !config.is_r10_frozen() {
            return Err(anyhow!(
                "hotword bias configuration is not the frozen R10 configuration"
            ));
        }
        if canonical_terms.is_empty() || canonical_terms.len() > R10_MAX_CANONICAL_TERMS {
            return Err(anyhow!(
                "hotword bias canonical term count is out of bounds"
            ));
        }
        let n_vocab = usize::try_from(context.n_vocab())?;
        if n_vocab == 0 {
            return Err(anyhow!("hotword bias model vocabulary is empty"));
        }

        let mut unique_terms = BTreeSet::new();
        let mut global_sequences = BTreeSet::<Vec<i32>>::new();
        let mut term_diagnostics = Vec::with_capacity(canonical_terms.len());
        for (canonical_index, term) in canonical_terms.iter().enumerate() {
            if term.is_empty()
                || term.trim() != term
                || term.chars().count() > R10_MAX_TERM_CHARS
                || !unique_terms.insert(term.clone())
            {
                return Err(anyhow!("hotword bias canonical term is invalid"));
            }
            let mut term_sequences = BTreeSet::<Vec<i32>>::new();
            for variant in [term.clone(), format!(" {term}")] {
                let tokens = context.tokenize(&variant, R10_TOKENIZE_CAPACITY)?;
                if tokens.is_empty()
                    || tokens.len() > R10_MAX_TOKENS_PER_SEQUENCE
                    || tokens.iter().any(|token| {
                        *token < 0 || usize::try_from(*token).map_or(true, |id| id >= n_vocab)
                    })
                {
                    return Err(anyhow!("hotword bias token sequence is invalid"));
                }
                term_sequences.insert(tokens);
            }
            if term_sequences.is_empty() {
                return Err(anyhow!("hotword bias term produced no token sequence"));
            }
            let token_counts = term_sequences.iter().map(Vec::len).collect::<Vec<_>>();
            let token_sequence_sha256 = term_sequences
                .iter()
                .map(sha256_json)
                .collect::<Result<Vec<_>>>()?;
            global_sequences.extend(term_sequences.iter().cloned());
            term_diagnostics.push(WhisperHotwordTermDiagnostics {
                canonical_index,
                term_sha256: sha256_bytes(term.as_bytes()),
                token_sequence_count: term_sequences.len(),
                token_counts,
                token_sequence_sha256,
            });
        }

        let sequences = global_sequences.into_iter().collect::<Vec<_>>();
        if sequences.is_empty() {
            return Err(anyhow!("hotword bias sequence set is empty"));
        }
        let canonical_terms_sha256 = sha256_json(&canonical_terms)?;
        let token_sequence_set_sha256 = sha256_json(&sequences)?;
        let binding = WhisperHotwordBiasBinding {
            schema_version: R10_HOTWORD_BIAS_SCHEMA_VERSION,
            strategy: R10_HOTWORD_BIAS_STRATEGY,
            canonical_terms_sha256: &canonical_terms_sha256,
            canonical_term_count: canonical_terms.len(),
            token_sequence_set_sha256: &token_sequence_set_sha256,
            token_sequence_count: sequences.len(),
            tokenization_variants: &R10_HOTWORD_TOKENIZATION_VARIANTS,
            maximum_tokens_per_sequence: R10_MAX_TOKENS_PER_SEQUENCE,
            config,
            duplicate_adjustment_rule: "maximum_not_sum",
            terms: &term_diagnostics,
        };
        let binding_sha256 = sha256_json(&binding)?;
        let base_diagnostics = WhisperHotwordBiasDiagnostics {
            schema_version: R10_HOTWORD_BIAS_SCHEMA_VERSION,
            strategy: R10_HOTWORD_BIAS_STRATEGY.to_owned(),
            binding_sha256,
            canonical_terms_sha256,
            canonical_term_count: canonical_terms.len(),
            token_sequence_set_sha256,
            token_sequence_count: sequences.len(),
            tokenization_variants: R10_HOTWORD_TOKENIZATION_VARIANTS
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            maximum_tokens_per_sequence: R10_MAX_TOKENS_PER_SEQUENCE,
            start_token_logit_add: config.start_token_logit_add,
            continuation_token_logit_add: config.continuation_token_logit_add,
            completion_token_logit_add: config.completion_token_logit_add,
            duplicate_adjustment_rule: "maximum_not_sum".to_owned(),
            canonical_term_plaintext_stored: false,
            raw_token_ids_stored: false,
            terms: term_diagnostics,
            decode_call_count: 1,
            callback_invocations: 0,
            callbacks_with_adjustments: 0,
            logit_adjustment_count: 0,
            maximum_adjustments_in_one_callback: 0,
        };
        Ok(Self {
            n_vocab,
            sequences,
            config,
            base_diagnostics,
            callback_invocations: AtomicU64::new(0),
            callbacks_with_adjustments: AtomicU64::new(0),
            logit_adjustment_count: AtomicU64::new(0),
            maximum_adjustments_in_one_callback: AtomicU64::new(0),
        })
    }

    pub(crate) fn install(&self, params: &mut FullParams<'_, '_>) {
        let user_data = (self as *const Self).cast_mut().cast::<c_void>();
        // SAFETY: `self` remains alive and at a stable address until the synchronous
        // `WhisperState::full` call returns. The callback treats it as immutable and
        // mutates only its atomics plus the logits buffer supplied by whisper.cpp.
        unsafe {
            params.set_filter_logits_callback(Some(hotword_logits_filter));
            params.set_filter_logits_callback_user_data(user_data);
        }
    }

    pub(crate) fn diagnostics(&self) -> WhisperHotwordBiasDiagnostics {
        let mut diagnostics = self.base_diagnostics.clone();
        diagnostics.callback_invocations = self.callback_invocations.load(Ordering::Relaxed);
        diagnostics.callbacks_with_adjustments =
            self.callbacks_with_adjustments.load(Ordering::Relaxed);
        diagnostics.logit_adjustment_count = self.logit_adjustment_count.load(Ordering::Relaxed);
        diagnostics.maximum_adjustments_in_one_callback = self
            .maximum_adjustments_in_one_callback
            .load(Ordering::Relaxed);
        diagnostics
    }

    fn planned_adjustments(&self, history: &[i32]) -> Vec<(usize, f32)> {
        let mut adjustments = BTreeMap::<usize, f32>::new();
        for sequence in &self.sequences {
            let start_boost = if sequence.len() == 1 {
                self.config.completion_token_logit_add
            } else {
                self.config.start_token_logit_add
            };
            record_max_adjustment(&mut adjustments, sequence[0], start_boost, self.n_vocab);

            if sequence.len() <= 1 || history.is_empty() {
                continue;
            }
            let maximum_prefix = history.len().min(sequence.len() - 1);
            for prefix_len in (1..=maximum_prefix).rev() {
                if history.ends_with(&sequence[..prefix_len]) {
                    let next_token = sequence[prefix_len];
                    let boost = if prefix_len + 1 == sequence.len() {
                        self.config.completion_token_logit_add
                    } else {
                        self.config.continuation_token_logit_add
                    };
                    record_max_adjustment(&mut adjustments, next_token, boost, self.n_vocab);
                    break;
                }
            }
        }
        adjustments.into_iter().collect()
    }

    fn apply(&self, history: &[i32], logits: &mut [f32]) {
        self.callback_invocations.fetch_add(1, Ordering::Relaxed);
        let adjustments = self.planned_adjustments(history);
        let mut applied = 0u64;
        for (token_id, boost) in adjustments {
            let Some(logit) = logits.get_mut(token_id) else {
                continue;
            };
            if !logit.is_finite() {
                continue;
            }
            let adjusted = *logit + boost;
            if adjusted.is_finite() {
                *logit = adjusted;
                applied += 1;
            }
        }
        if applied > 0 {
            self.callbacks_with_adjustments
                .fetch_add(1, Ordering::Relaxed);
            self.logit_adjustment_count
                .fetch_add(applied, Ordering::Relaxed);
            self.maximum_adjustments_in_one_callback
                .fetch_max(applied, Ordering::Relaxed);
        }
    }
}

fn record_max_adjustment(
    adjustments: &mut BTreeMap<usize, f32>,
    token_id: i32,
    boost: f32,
    n_vocab: usize,
) {
    let Ok(token_id) = usize::try_from(token_id) else {
        return;
    };
    if token_id >= n_vocab || !boost.is_finite() || boost <= 0.0 {
        return;
    }
    adjustments
        .entry(token_id)
        .and_modify(|current| *current = current.max(boost))
        .or_insert(boost);
}

unsafe extern "C" fn hotword_logits_filter(
    _context: *mut WhisperSysContext,
    _state: *mut WhisperSysState,
    tokens: *const WhisperTokenData,
    n_tokens: c_int,
    logits: *mut f32,
    user_data: *mut c_void,
) {
    if user_data.is_null() || logits.is_null() || n_tokens < 0 {
        return;
    }
    let n_tokens = n_tokens as usize;
    if n_tokens > MAX_CALLBACK_HISTORY_TOKENS || (n_tokens > 0 && tokens.is_null()) {
        return;
    }
    // No panic is allowed to cross the C ABI. All pointers originate from the
    // synchronous install/full pair above and every length is bounded first.
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let bias = unsafe { &*(user_data.cast::<WhisperHotwordBiasState>()) };
        let logits = unsafe { std::slice::from_raw_parts_mut(logits, bias.n_vocab) };
        let history = if n_tokens == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(tokens, n_tokens) }
                .iter()
                .map(|token| token.id)
                .collect::<Vec<_>>()
        };
        bias.apply(&history, logits);
    }));
}

fn sha256_json<T: Serialize>(value: &T) -> Result<String> {
    Ok(sha256_bytes(&serde_json::to_vec(value)?))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(sequences: Vec<Vec<i32>>) -> WhisperHotwordBiasState {
        let config = WhisperHotwordBiasConfig::r10_frozen();
        WhisperHotwordBiasState {
            n_vocab: 100,
            sequences,
            config,
            base_diagnostics: WhisperHotwordBiasDiagnostics {
                schema_version: R10_HOTWORD_BIAS_SCHEMA_VERSION,
                strategy: R10_HOTWORD_BIAS_STRATEGY.to_owned(),
                binding_sha256: "a".repeat(64),
                canonical_terms_sha256: "b".repeat(64),
                canonical_term_count: 1,
                token_sequence_set_sha256: "c".repeat(64),
                token_sequence_count: 1,
                tokenization_variants: vec!["canonical".to_owned()],
                maximum_tokens_per_sequence: R10_MAX_TOKENS_PER_SEQUENCE,
                start_token_logit_add: config.start_token_logit_add,
                continuation_token_logit_add: config.continuation_token_logit_add,
                completion_token_logit_add: config.completion_token_logit_add,
                duplicate_adjustment_rule: "maximum_not_sum".to_owned(),
                canonical_term_plaintext_stored: false,
                raw_token_ids_stored: false,
                terms: Vec::new(),
                decode_call_count: 1,
                callback_invocations: 0,
                callbacks_with_adjustments: 0,
                logit_adjustment_count: 0,
                maximum_adjustments_in_one_callback: 0,
            },
            callback_invocations: AtomicU64::new(0),
            callbacks_with_adjustments: AtomicU64::new(0),
            logit_adjustment_count: AtomicU64::new(0),
            maximum_adjustments_in_one_callback: AtomicU64::new(0),
        }
    }

    #[test]
    fn suffix_bias_uses_frozen_weights_and_maximum_not_sum() {
        let bias = state(vec![vec![10, 20, 30], vec![10, 20, 40], vec![50]]);
        assert_eq!(bias.planned_adjustments(&[]), vec![(10, 1.5), (50, 4.0)]);
        assert_eq!(
            bias.planned_adjustments(&[7, 10]),
            vec![(10, 1.5), (20, 3.0), (50, 4.0)]
        );
        assert_eq!(
            bias.planned_adjustments(&[7, 10, 20]),
            vec![(10, 1.5), (30, 4.0), (40, 4.0), (50, 4.0)]
        );
    }

    #[test]
    fn ffi_callback_only_changes_planned_finite_logits() {
        let bias = Box::new(state(vec![vec![10, 20, 30]]));
        let token = WhisperTokenData {
            id: 10,
            tid: 10,
            p: 0.0,
            plog: 0.0,
            pt: 0.0,
            ptsum: 0.0,
            t0: 0,
            t1: 0,
            t_dtw: 0,
            vlen: 0.0,
        };
        let mut logits = vec![0.0f32; 100];
        logits[10] = f32::NEG_INFINITY;
        unsafe {
            hotword_logits_filter(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &token,
                1,
                logits.as_mut_ptr(),
                ((&*bias) as *const WhisperHotwordBiasState)
                    .cast_mut()
                    .cast(),
            );
        }
        assert_eq!(logits[10], f32::NEG_INFINITY);
        assert_eq!(logits[20], 3.0);
        assert_eq!(logits[30], 0.0);
        let diagnostics = bias.diagnostics();
        assert_eq!(diagnostics.callback_invocations, 1);
        assert_eq!(diagnostics.callbacks_with_adjustments, 1);
        assert_eq!(diagnostics.logit_adjustment_count, 1);
    }

    #[test]
    fn ffi_callback_rejects_invalid_inputs_without_mutation() {
        let bias = Box::new(state(vec![vec![10]]));
        let mut logits = vec![0.0f32; 100];
        unsafe {
            hotword_logits_filter(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
                -1,
                logits.as_mut_ptr(),
                ((&*bias) as *const WhisperHotwordBiasState)
                    .cast_mut()
                    .cast(),
            );
            hotword_logits_filter(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
                1,
                logits.as_mut_ptr(),
                ((&*bias) as *const WhisperHotwordBiasState)
                    .cast_mut()
                    .cast(),
            );
        }
        assert!(logits.iter().all(|value| *value == 0.0));
        assert_eq!(bias.diagnostics().callback_invocations, 0);
    }

    #[test]
    fn diagnostics_merge_requires_identical_binding() {
        let mut first = state(vec![vec![10]]).diagnostics();
        let mut second = first.clone();
        second.decode_call_count = 2;
        second.callback_invocations = 5;
        second.callbacks_with_adjustments = 4;
        second.logit_adjustment_count = 9;
        second.maximum_adjustments_in_one_callback = 3;
        first.merge_decode(&second).unwrap();
        assert_eq!(first.decode_call_count, 3);
        assert_eq!(first.callback_invocations, 5);
        assert_eq!(first.logit_adjustment_count, 9);
        assert_eq!(first.maximum_adjustments_in_one_callback, 3);

        second.binding_sha256 = "d".repeat(64);
        assert!(first.merge_decode(&second).is_err());
    }
}
