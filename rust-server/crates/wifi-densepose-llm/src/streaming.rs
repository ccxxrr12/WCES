//! Streaming LLM Generation Engine
//!
//! Loads a Qwen2.5-0.5B GGUF model via candle 0.8+ and generates tokens
//! one at a time, pushing each through a broadcast channel for
//! real-time display in the triage UI.
//!
//! Only compiled when the `llm` feature is enabled.
//!
//! Based on the official candle quantized-qwen2 example:
//! https://github.com/huggingface/candle/blob/main/candle-examples/examples/quantized-qwen2-instruct/main.rs

use anyhow::{Context, Result};
use candle_core::quantized::gguf_file;
use candle_core::{Device, Tensor};
use candle_transformers::generation::LogitsProcessor;
use candle_transformers::models::quantized_qwen2::ModelWeights as Qwen2;
use std::path::Path;
use std::sync::Arc;
use tokenizers::Tokenizer;
use tokio::sync::Mutex;

use crate::types::{LlmGenerationResult, StreamToken};

// ── Streaming Generator ─────────────────────────────────────────────────────

/// Streaming LLM generator using quantized Qwen2 GGUF models.
pub struct StreamingGenerator {
    model: Qwen2,
    tokenizer: Tokenizer,
    device: Device,
    eos_token_id: u32,
}

impl StreamingGenerator {
    /// Load a Qwen2 GGUF model from disk.
    ///
    /// # Arguments
    /// * `model_path` - Path to `.gguf` model file (e.g. Qwen2.5-0.5B Q4_0)
    /// * `tokenizer_path` - Path to `tokenizer.json`
    /// * `cpu` - Force CPU inference (set to true for RZ/V2H ARM64)
    pub fn load(
        model_path: impl AsRef<Path>,
        tokenizer_path: impl AsRef<Path>,
        cpu: bool,
    ) -> Result<Self> {
        let model_path = model_path.as_ref();
        let tokenizer_path = tokenizer_path.as_ref();

        tracing::info!("Loading GGUF model from: {}", model_path.display());

        // Use CPU on RZ/V2H
        let device = if cpu {
            Device::Cpu
        } else {
            Device::new_metal(0)
                .or_else(|_| Device::new_cuda(0))
                .unwrap_or(Device::Cpu)
        };

        // 1. Read GGUF file
        let mut file =
            std::fs::File::open(model_path).context("Failed to open GGUF model file")?;
        let model_content = gguf_file::Content::read(&mut file)
            .map_err(|e| anyhow::anyhow!("Failed to parse GGUF: {}", e))?;

        let tensor_count = model_content.tensor_infos.len();
        tracing::info!(
            "GGUF loaded: {} tensors, metadata entries: {}",
            tensor_count,
            model_content.metadata.len()
        );

        // 2. Build quantized model
        let model = Qwen2::from_gguf(model_content, &mut file, &device)
            .context("Failed to build Qwen2 model from GGUF")?;

        tracing::info!("Qwen2 model built successfully on {:?}", device);

        // 3. Load tokenizer
        let tokenizer =
            Tokenizer::from_file(tokenizer_path).map_err(anyhow::Error::msg)?;

        tracing::info!(
            "Tokenizer loaded: vocab_size={}",
            tokenizer.get_vocab_size(true)
        );

        // 4. Determine EOS token
        // Qwen2 typically uses <|im_end|> as EOS
        let eos_token_id = tokenizer
            .token_to_id("<|im_end|>")
            .or_else(|| tokenizer.token_to_id("<|endoftext|>"))
            .unwrap_or(151643); // Qwen default EOS

        Ok(Self {
            model,
            tokenizer,
            device,
            eos_token_id,
        })
    }

    /// Get the vocabulary size.
    pub fn vocab_size(&self) -> usize {
        self.tokenizer.get_vocab_size(true)
    }

    /// Generate tokens in a streaming fashion, pushing each via the broadcast channel.
    pub fn generate_stream(
        &mut self,
        prompt: &str,
        survivor_id: &str,
        max_new_tokens: usize,
        temperature: f64,
        seed: u64,
        tx: tokio::sync::broadcast::Sender<StreamToken>,
    ) -> Result<LlmGenerationResult> {
        let start = std::time::Instant::now();

        // ── Tokenize prompt ───────────────────────────────────────
        let tokens = self
            .tokenizer
            .encode(prompt, true)
            .map_err(anyhow::Error::msg)
            .context("Failed to tokenize prompt")?;

        let prompt_tokens = tokens.get_ids().len();
        let mut token_ids = tokens.get_ids().to_vec();

        tracing::info!(
            "Starting generation: {} prompt tokens, max {} new tokens, temp {:.2}",
            prompt_tokens,
            max_new_tokens,
            temperature
        );

        // ── Setup logits processor ───────────────────────────────
        let mut logits_processor = {
            let temperature = if temperature <= 0.0 { None } else { Some(temperature) };
            let sampling = if temperature.is_some() {
                candle_transformers::generation::Sampling::All { temperature: temperature.unwrap() }
            } else {
                candle_transformers::generation::Sampling::ArgMax
            };
            LogitsProcessor::from_sampling(seed, sampling)
        };

        // ── Generation loop ───────────────────────────────────────
        let mut generated_tokens = 0usize;
        let mut full_text = String::new();
        let mut index_pos = 0usize;

        for index in 0..max_new_tokens {
            // Build input tensor: use a context window of the last N tokens
            let context_size = std::cmp::min(2048, token_ids.len());
            let start_idx = token_ids.len() - context_size;
            let context = &token_ids[start_idx..];

            let input = Tensor::new(context, &self.device)
                .context("Failed to create input tensor")?
                .unsqueeze(0)?;

            // Forward pass
            let logits = self.model.forward(&input, index_pos)?;

            // Get logits for the last position
            let logits = logits.squeeze(0)?;
            let last_logits = logits.get(logits.dim(0)? - 1)?;

            // Sample next token
            let next_token = logits_processor.sample(&last_logits)?;

            // Check for EOS
            if next_token == self.eos_token_id as u32 {
                let _ = tx.send(StreamToken {
                    survivor_id: survivor_id.to_string(),
                    token_index: index as u32,
                    text: String::new(),
                    is_complete: true,
                });
                tracing::info!(
                    "Generation complete (EOS): {} tokens in {:.2}s",
                    generated_tokens,
                    start.elapsed().as_secs_f64()
                );
                break;
            }

            // Decode this token
            let token_text = self
                .tokenizer
                .decode(&[next_token], false)
                .map_err(anyhow::Error::msg)
                .unwrap_or_else(|_| "�".to_string());

            full_text.push_str(&token_text);

            // Push through broadcast channel
            let _ = tx.send(StreamToken {
                survivor_id: survivor_id.to_string(),
                token_index: index as u32,
                text: token_text.clone(),
                is_complete: false,
            });

            // Append token and advance position
            token_ids.push(next_token);
            index_pos += context_size;
            generated_tokens += 1;
        }

        // Send completion signal
        let _ = tx.send(StreamToken {
            survivor_id: survivor_id.to_string(),
            token_index: u32::MAX,
            text: String::new(),
            is_complete: true,
        });

        let elapsed = start.elapsed();

        Ok(LlmGenerationResult {
            survivor_id: survivor_id.to_string(),
            full_text,
            generated_tokens,
            elapsed_ms: elapsed.as_millis() as u64,
            prompt_tokens,
        })
    }

    /// Non-streaming generation — returns the full response.
    pub fn generate(
        &mut self,
        prompt: &str,
        max_new_tokens: usize,
        temperature: f64,
        seed: u64,
    ) -> Result<LlmGenerationResult> {
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        self.generate_stream(prompt, "__sync__", max_new_tokens, temperature, seed, tx)
    }
}

// ── Dedicated LLM Runtime ────────────────────────────────────────────────────

/// Dedicated runtime for hosting the LLM in its own tokio task.
///
/// Inference can take 30-120 seconds on RZ/V2H ARM64.
/// This runs on a dedicated blocking thread to avoid starving
/// the sensing-server's main async loop.
pub struct LlmRuntime {
    generator: Arc<Mutex<StreamingGenerator>>,
}

impl LlmRuntime {
    /// Create a new LLM runtime from an already-loaded generator.
    pub fn new(generator: StreamingGenerator) -> Self {
        Self {
            generator: Arc::new(Mutex::new(generator)),
        }
    }

    /// Spawn an async generation task.
    ///
    /// Returns the receiver end of the broadcast channel so callers
    /// can consume tokens as they arrive.
    pub async fn spawn_generation(
        &self,
        prompt: String,
        survivor_id: String,
        max_new_tokens: usize,
        temperature: f64,
    ) -> (
        tokio::sync::broadcast::Receiver<StreamToken>,
        tokio::task::JoinHandle<Result<LlmGenerationResult>>,
    ) {
        let generator = self.generator.clone();
        let (tx, rx) = tokio::sync::broadcast::channel(64);

        let handle = tokio::task::spawn_blocking(move || {
            let mut gen = generator.blocking_lock();
            let seed = 299792458u64;
            gen.generate_stream(
                &prompt,
                &survivor_id,
                max_new_tokens,
                temperature,
                seed,
                tx,
            )
        });

        (rx, handle)
    }
}
