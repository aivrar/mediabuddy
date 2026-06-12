//! Florence-2 inference engine: 4 ONNX sessions + tokenizer.
//!
//! Pipeline for any task token (`<CAPTION>`, `<OD>`, `<OCR>`, …):
//! 1. Resize+normalize image to NCHW float32 (1,3,768,768).
//! 2. `vision_encoder`(pixel_values) -> visual token embeds (1,V,768).
//! 3. Tokenize task prompt -> input_ids; `embed_tokens`(input_ids) ->
//!    text token embeds (1,T,768).
//! 4. Concat [visual, text] along seq -> inputs_embeds (1,V+T,768).
//! 5. `encoder_model`(inputs_embeds, attention_mask) -> hidden_states.
//! 6. Greedy decode loop: embed the decoder token prefix, then run
//!    `decoder_model` (no KV cache; each step re-runs over the full prefix).
//!    Stops on EOS or max length.
//! 7. Decode generated token IDs back to text.
//!
//! For OD / region tasks the raw text contains `<loc_NNN>` tokens
//! representing 0..999 normalized coords; [`parse_od_string`] converts
//! them into pixel-space [x1,y1,x2,y2] boxes.
//!
//! ort is used through its tuple-data API (`(shape, Vec<T>)`) rather
//! than the `ndarray` feature, because that feature drags in C++ code
//! referencing MSVC STL intrinsics that aren't always available in the
//! shipped onnxruntime prebuilds.

use std::path::Path;
use std::sync::Mutex;

use ndarray::{s, Array1, Array3, Array4, Axis, Ix3};
use ort::execution_providers::{CUDAExecutionProvider, DirectMLExecutionProvider};
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use regex::Regex;
use tokenizers::Tokenizer;

use crate::error::{AppError, Result};
use crate::vision::preprocess::{preprocess_image, PreprocessConfig};
use crate::vision::{
    download::ModelPaths, DetectedObject, EngineConfig, ExecutionTarget, Precision,
    VisionInstanceStatus,
};

/// BART-style: decoder starts with </s> (id 2). EOS is also 2; PAD is 1.
const DECODER_START_TOKEN_ID: i64 = 2;
const EOS_TOKEN_ID: i64 = 2;
const MAX_NEW_TOKENS: usize = 256;

pub struct VisionEngine {
    index: usize,
    target: ExecutionTarget,
    intra_threads: usize,
    vision_encoder: Mutex<Session>,
    embed_tokens: Mutex<Session>,
    encoder: Mutex<Session>,
    decoder: Mutex<Session>,
    tokenizer: Tokenizer,
    #[allow(dead_code)]
    precision: Precision,
}

impl VisionEngine {
    pub fn open(paths: &ModelPaths, precision: Precision, config: EngineConfig) -> Result<Self> {
        let make = |label: &str, p: &Path| -> Result<Session> {
            tracing::info!(
                "ort: opening {label} on {} at {}",
                config.target.provider(),
                p.display()
            );
            let mut builder = Session::builder()
                .map_err(|e| AppError::other(format!("ort builder ({label}): {e}")))?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(|e| AppError::other(format!("ort opt ({label}): {e}")))?
                .with_intra_threads(config.intra_threads)
                .map_err(|e| AppError::other(format!("ort threads ({label}): {e}")))?;

            match &config.target {
                ExecutionTarget::Cpu => {}
                ExecutionTarget::DirectMl { device_id, .. } => {
                    builder = builder
                        .with_memory_pattern(false)
                        .map_err(|e| {
                            AppError::other(format!("ort dml mem pattern ({label}): {e}"))
                        })?
                        .with_parallel_execution(false)
                        .map_err(|e| {
                            AppError::other(format!("ort dml execution mode ({label}): {e}"))
                        })?
                        .with_execution_providers([DirectMLExecutionProvider::default()
                            .with_device_id(*device_id as i32)
                            .build()
                            .error_on_failure()])
                        .map_err(|e| {
                            AppError::other(format!("ort directml provider ({label}): {e}"))
                        })?;
                }
                ExecutionTarget::Cuda { device_id, .. } => {
                    builder = builder
                        .with_execution_providers([CUDAExecutionProvider::default()
                            .with_device_id(*device_id as i32)
                            .build()
                            .error_on_failure()])
                        .map_err(|e| {
                            AppError::other(format!("ort cuda provider ({label}): {e}"))
                        })?;
                }
            }

            builder
                .commit_from_file(p)
                .map_err(|e| AppError::other(format!("ort load {label}: {e}")))
        };

        let vision_encoder = make("vision_encoder", &paths.vision_encoder)?;
        let embed_tokens = make("embed_tokens", &paths.embed_tokens)?;
        let encoder = make("encoder_model", &paths.encoder_model)?;
        let decoder = make("decoder_model", &paths.decoder_model)?;

        let tokenizer = Tokenizer::from_file(&paths.tokenizer)
            .map_err(|e| AppError::other(format!("tokenizer load: {e}")))?;

        Ok(Self {
            index: config.index,
            target: config.target,
            intra_threads: config.intra_threads,
            vision_encoder: Mutex::new(vision_encoder),
            embed_tokens: Mutex::new(embed_tokens),
            encoder: Mutex::new(encoder),
            decoder: Mutex::new(decoder),
            tokenizer,
            precision,
        })
    }

    pub fn status(&self) -> VisionInstanceStatus {
        VisionInstanceStatus {
            index: self.index,
            provider: self.target.provider().to_string(),
            device_id: self.target.device_id(),
            device_name: self.target.device_name().map(str::to_string),
            precision: format!("{:?}", self.precision).to_lowercase(),
            intra_threads: self.intra_threads,
        }
    }

    pub fn caption(&self, image_path: &Path, task: &str) -> Result<String> {
        let raw = self.generate(image_path, task)?;
        Ok(strip_special_tokens(&raw))
    }

    pub fn detect_objects(&self, image_path: &Path, task: &str) -> Result<Vec<DetectedObject>> {
        let raw = self.generate(image_path, task)?;
        let img = image::open(image_path)?;
        Ok(parse_od_string(
            &raw,
            img.width() as f32,
            img.height() as f32,
        ))
    }

    fn generate(&self, image_path: &Path, task: &str) -> Result<String> {
        // 1. Preprocess image -> Array4 (1,3,768,768) f32
        let pixel_values: Array4<f32> = preprocess_image(image_path, PreprocessConfig::default())?;

        // 2. Vision encoder -> Array3 (1, V, 768)
        let visual_embeds = self.run_vision_encoder(pixel_values)?;

        // 3. Tokenize prompt
        let prompt = construct_prompt(task);
        let encoding = self
            .tokenizer
            .encode(prompt, true)
            .map_err(|e| AppError::other(format!("tokenize prompt: {e}")))?;
        let prompt_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let prompt_len = prompt_ids.len();

        // 4. Embed prompt -> Array3 (1, T, 768)
        let text_embeds = self.run_embed_tokens(&prompt_ids, prompt_len)?;

        // 5. Concat [visual; text] -> Array3 (1, V+T, 768)
        let inputs_embeds_dyn =
            ndarray::concatenate(Axis(1), &[visual_embeds.view(), text_embeds.view()])
                .map_err(|e| AppError::other(format!("concat embeds: {e}")))?;
        let inputs_embeds: Array3<f32> = inputs_embeds_dyn
            .into_dimensionality::<Ix3>()
            .map_err(|e| AppError::other(format!("inputs_embeds dim: {e}")))?;
        let total_seq = inputs_embeds.shape()[1];

        // 6. BART encoder -> Array3 (1, V+T, 768)
        let encoder_hidden = self.run_encoder(inputs_embeds, total_seq)?;

        // 7. Greedy decode (no cache; re-run over full prefix each step)
        let mut generated: Vec<i64> = vec![DECODER_START_TOKEN_ID];
        for _ in 0..MAX_NEW_TOKENS {
            let logits = self.run_decoder(&generated, &encoder_hidden, total_seq)?;
            // Guard against a zero-length sequence dim: `shape[1] - 1` would
            // underflow usize and panic (whole-process abort under panic=abort).
            let seq_out = logits.shape()[1];
            if seq_out == 0 {
                return Err(AppError::other("decoder produced an empty sequence"));
            }
            let last = logits.slice(s![0, seq_out - 1, ..]).to_owned();
            let next_id = argmax_f32(&last);
            generated.push(next_id);
            if next_id == EOS_TOKEN_ID {
                break;
            }
        }

        // 8. Decode token IDs -> text. Keep special tokens so OD/region
        //    parsers can find <loc_NNN> markers; caption() strips them.
        let token_ids: Vec<u32> = generated
            .iter()
            .skip(1) // drop the decoder_start token
            .filter(|&&id| id != EOS_TOKEN_ID)
            .map(|&id| id as u32)
            .collect();
        let text = self
            .tokenizer
            .decode(&token_ids, false)
            .map_err(|e| AppError::other(format!("detokenize: {e}")))?;
        Ok(text)
    }

    fn run_vision_encoder(&self, pixel_values: Array4<f32>) -> Result<Array3<f32>> {
        let shape: Vec<i64> = pixel_values.shape().iter().map(|&d| d as i64).collect();
        let (data, _) = pixel_values.into_raw_vec_and_offset();
        let mut sess = self
            .vision_encoder
            .lock()
            .map_err(|_| AppError::other("vision_encoder session lock poisoned"))?;
        let outputs = sess
            .run(ort::inputs![
                "pixel_values" => Tensor::from_array((shape, data))
                    .map_err(|e| AppError::other(format!("pixel tensor: {e}")))?,
            ])
            .map_err(|e| AppError::other(format!("vision_encoder run: {e}")))?;
        extract_3d(&outputs[0])
    }

    fn run_embed_tokens(&self, ids: &[i64], len: usize) -> Result<Array3<f32>> {
        let shape: Vec<i64> = vec![1, len as i64];
        let mut sess = self
            .embed_tokens
            .lock()
            .map_err(|_| AppError::other("embed_tokens session lock poisoned"))?;
        let outputs = sess
            .run(ort::inputs![
                "input_ids" => Tensor::from_array((shape, ids.to_vec()))
                    .map_err(|e| AppError::other(format!("ids tensor: {e}")))?,
            ])
            .map_err(|e| AppError::other(format!("embed_tokens run: {e}")))?;
        extract_3d(&outputs[0])
    }

    fn run_encoder(&self, inputs_embeds: Array3<f32>, seq: usize) -> Result<Array3<f32>> {
        let embeds_shape: Vec<i64> = inputs_embeds.shape().iter().map(|&d| d as i64).collect();
        let (embeds_data, _) = inputs_embeds.into_raw_vec_and_offset();
        let mask: Vec<i64> = vec![1; seq];
        let mask_shape: Vec<i64> = vec![1, seq as i64];
        let mut sess = self
            .encoder
            .lock()
            .map_err(|_| AppError::other("encoder session lock poisoned"))?;
        let outputs = sess
            .run(ort::inputs![
                "inputs_embeds" => Tensor::from_array((embeds_shape, embeds_data))
                    .map_err(|e| AppError::other(format!("inputs_embeds tensor: {e}")))?,
                "attention_mask" => Tensor::from_array((mask_shape, mask))
                    .map_err(|e| AppError::other(format!("attention_mask tensor: {e}")))?,
            ])
            .map_err(|e| AppError::other(format!("encoder run: {e}")))?;
        extract_3d(&outputs[0])
    }

    fn run_decoder(
        &self,
        decoder_ids: &[i64],
        encoder_hidden: &Array3<f32>,
        encoder_seq: usize,
    ) -> Result<Array3<f32>> {
        if decoder_ids.is_empty() {
            return Err(AppError::other("decoder token prefix is empty"));
        }
        let decoder_embeds = self.run_embed_tokens(decoder_ids, decoder_ids.len())?;
        let decoder_shape: Vec<i64> = decoder_embeds.shape().iter().map(|&d| d as i64).collect();
        let (decoder_data, _) = decoder_embeds.into_raw_vec_and_offset();
        let enc_mask: Vec<i64> = vec![1; encoder_seq];
        let enc_mask_shape: Vec<i64> = vec![1, encoder_seq as i64];
        let enc_shape: Vec<i64> = encoder_hidden.shape().iter().map(|&d| d as i64).collect();
        let enc_data: Vec<f32> = encoder_hidden.iter().copied().collect();

        let mut sess = self
            .decoder
            .lock()
            .map_err(|_| AppError::other("decoder session lock poisoned"))?;
        let outputs = sess
            .run(ort::inputs![
                "encoder_attention_mask" => Tensor::from_array((enc_mask_shape, enc_mask))
                    .map_err(|e| AppError::other(format!("dec mask tensor: {e}")))?,
                "encoder_hidden_states" => Tensor::from_array((enc_shape, enc_data))
                    .map_err(|e| AppError::other(format!("enc hidden tensor: {e}")))?,
                "inputs_embeds" => Tensor::from_array((decoder_shape, decoder_data))
                    .map_err(|e| AppError::other(format!("dec embeds tensor: {e}")))?,
            ])
            .map_err(|e| AppError::other(format!("decoder run: {e}")))?;
        extract_3d(&outputs[0])
    }
}

/// Extract a 3D f32 tensor output as an owned `Array3<f32>`.
fn extract_3d(value: &ort::value::DynValue) -> Result<Array3<f32>> {
    let (shape, data) = value
        .try_extract_tensor::<f32>()
        .map_err(|e| AppError::other(format!("extract tensor: {e}")))?;
    if shape.len() != 3 {
        return Err(AppError::other(format!(
            "expected 3-D output tensor, got shape {shape:?}"
        )));
    }
    let dims = (shape[0] as usize, shape[1] as usize, shape[2] as usize);
    Array3::from_shape_vec(dims, data.to_vec())
        .map_err(|e| AppError::other(format!("rebuild Array3: {e}")))
}

fn argmax_f32(arr: &Array1<f32>) -> i64 {
    let mut best_i = 0i64;
    let mut best_v = f32::NEG_INFINITY;
    for (i, &v) in arr.iter().enumerate() {
        if v > best_v {
            best_v = v;
            best_i = i as i64;
        }
    }
    best_i
}

fn strip_special_tokens(s: &str) -> String {
    let re = Regex::new(r"<[^>]+>").unwrap();
    re.replace_all(s, " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn construct_prompt(task: &str) -> &str {
    match task {
        "<CAPTION>" => "What does the image describe?",
        "<DETAILED_CAPTION>" => "Describe in detail what is shown in the image.",
        "<MORE_DETAILED_CAPTION>" => "Describe with a paragraph what is shown in the image.",
        "<OD>" => "Locate the objects with category name in the image.",
        "<DENSE_REGION_CAPTION>" => "Locate the objects in the image, with their descriptions.",
        "<REGION_PROPOSAL>" => "Locate the region proposals in the image.",
        "<OCR>" => "What is the text in the image?",
        other => other,
    }
}

/// Parse Florence-2 OD output. Format: a sequence of
/// `LABEL<loc_x1><loc_y1><loc_x2><loc_y2>` records.
/// Coords are integers 0..999 mapping to normalized image space.
fn parse_od_string(s: &str, img_w: f32, img_h: f32) -> Vec<DetectedObject> {
    let re = Regex::new(r"([^<>]+)<loc_(\d+)><loc_(\d+)><loc_(\d+)><loc_(\d+)>").unwrap();
    let mut out = Vec::new();
    for cap in re.captures_iter(s) {
        let label = cap
            .get(1)
            .map(|m| m.as_str().trim())
            .unwrap_or("")
            .to_string();
        if label.is_empty() {
            continue;
        }
        let p = |i: usize| {
            cap.get(i)
                .and_then(|m| m.as_str().parse::<u32>().ok())
                .unwrap_or(0)
        };
        let (x1, y1, x2, y2) = (p(2), p(3), p(4), p(5));
        let to_px_x = |v: u32| (v as f32 / 999.0) * img_w;
        let to_px_y = |v: u32| (v as f32 / 999.0) * img_h;
        out.push(DetectedObject {
            label,
            bbox: [to_px_x(x1), to_px_y(y1), to_px_x(x2), to_px_y(y2)],
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array1;

    #[test]
    fn argmax_finds_max_index() {
        let a = Array1::from(vec![0.1, 0.3, 0.9, 0.4]);
        assert_eq!(argmax_f32(&a), 2);
    }

    #[test]
    fn strip_special_tokens_keeps_words() {
        let s = "<s>a cat sitting<loc_100><loc_200></s>";
        assert_eq!(strip_special_tokens(s), "a cat sitting");
    }

    #[test]
    fn construct_prompt_maps_task_tokens() {
        assert_eq!(
            construct_prompt("<DETAILED_CAPTION>"),
            "Describe in detail what is shown in the image."
        );
        assert_eq!(
            construct_prompt("<OD>"),
            "Locate the objects with category name in the image."
        );
        assert_eq!(construct_prompt("custom prompt"), "custom prompt");
    }

    #[test]
    fn parse_od_extracts_bboxes() {
        let s = "car<loc_100><loc_200><loc_300><loc_400>person<loc_50><loc_50><loc_500><loc_900>";
        let objs = parse_od_string(s, 1000.0, 1000.0);
        assert_eq!(objs.len(), 2);
        assert_eq!(objs[0].label, "car");
        assert_eq!(objs[1].label, "person");
        assert!((objs[0].bbox[0] - 100.1).abs() < 1.0);
    }
}
