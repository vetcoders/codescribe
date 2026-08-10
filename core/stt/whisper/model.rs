//! Whisper encoder/decoder layers — a local port of the reference implementation.
//!
//! The hyperparameters still come from upstream (`candle_transformers::models::
//! whisper::Config`) so HuggingFace checkpoints parse unchanged; only the layers
//! and the forward path live here. That split is what the port buys: every
//! `forward` carries a `flush_kv_cache` flag down to the attention blocks, so the
//! streaming decoder can invalidate cross-attention state when it moves to a new
//! audio window instead of rebuilding the whole model.
//!
//! Each struct keeps a permalink to the corresponding block in OpenAI's
//! `whisper/model.py`; those comments are the reference for layer shapes and
//! weight names, and should survive any refactor here.
//!
//! Weights are named for the HuggingFace Transformers layout (`q_proj`, `fc1`,
//! `embed_positions.weight`, …), not OpenAI's original checkpoint names.

use candle_core::{D, DType, Device, IndexOp, Result, Tensor};
use candle_nn::Linear;
use candle_nn::{Conv1d, Conv1dConfig, Embedding, LayerNorm, Module, VarBuilder, embedding};
use candle_transformers::models::whisper::Config;

/// Load a bias-carrying projection from `weight` + `bias` under `vb`.
fn linear(in_dim: usize, out_dim: usize, vb: VarBuilder) -> Result<Linear> {
    let weight = vb.get((out_dim, in_dim), "weight")?;
    let bias = vb.get(out_dim, "bias")?;
    Ok(Linear::new(weight, Some(bias)))
}

/// Load a projection that has no bias term. Whisper's key projection is the
/// only attention projection trained without one, so it cannot share [`linear`].
fn linear_no_bias(in_dim: usize, out_dim: usize, vb: VarBuilder) -> Result<Linear> {
    let weight = vb.get((out_dim, in_dim), "weight")?;
    Ok(Linear::new(weight, None))
}

/// Load one of the encoder's two front-end convolutions from `vb`.
fn conv1d(
    in_channels: usize,
    out_channels: usize,
    kernel_size: usize,
    config: Conv1dConfig,
    vb: VarBuilder,
) -> Result<Conv1d> {
    let weight = vb.get((out_channels, in_channels, kernel_size), "weight")?;
    let bias = vb.get(out_channels, "bias")?;
    Ok(Conv1d::new(weight, Some(bias), config))
}

/// Load a layer norm at Whisper's fixed `1e-5` epsilon.
fn layer_norm(size: usize, vb: VarBuilder) -> Result<LayerNorm> {
    let weight = vb.get(size, "weight")?;
    let bias = vb.get(size, "bias")?;
    Ok(LayerNorm::new(weight, bias, 1e-5))
}

/// Multi-head attention serving both self- and cross-attention.
///
/// `kv_cache` only ever holds **cross-attention** keys and values: the encoder
/// output is constant for a whole decode window, so K/V are computed once and
/// reused, while self-attention recomputes them from the growing token prefix
/// every step. Moving to a new audio window must therefore flush the cache —
/// see the `flush_cache` flag on `forward`.
///
/// The `tracing` spans are permanent instrumentation: they are how the Metal vs
/// CPU cost split (softmax vs matmul) is measured.
// https://github.com/openai/whisper/blob/f572f2161ba831bae131364c3bffdead7af6d210/whisper/model.py#L62
#[derive(Debug, Clone)]
pub struct MultiHeadAttention {
    query: Linear,
    key: Linear,
    value: Linear,
    out: Linear,
    n_head: usize,
    span: tracing::Span,
    softmax_span: tracing::Span,
    matmul_span: tracing::Span,
    kv_cache: Option<(Tensor, Tensor)>,
}

impl MultiHeadAttention {
    /// Load the four projections (`q/k/v/out_proj`) for one attention module.
    /// Starts with an empty KV cache.
    pub fn load(n_state: usize, n_head: usize, vb: VarBuilder) -> Result<Self> {
        let span = tracing::span!(tracing::Level::TRACE, "multi-head-attn");
        let softmax_span = tracing::span!(tracing::Level::TRACE, "multi-head-attn-softmax");
        let matmul_span = tracing::span!(tracing::Level::TRACE, "multi-head-attn-matmul");
        let query = linear(n_state, n_state, vb.pp("q_proj"))?;
        let value = linear(n_state, n_state, vb.pp("v_proj"))?;
        let key = linear_no_bias(n_state, n_state, vb.pp("k_proj"))?;
        let out = linear(n_state, n_state, vb.pp("out_proj"))?;
        Ok(Self {
            query,
            key,
            value,
            out,
            n_head,
            span,
            softmax_span,
            matmul_span,
            kv_cache: None,
        })
    }

    /// Attend over `x`, cross-attending to `xa` when the caller supplies encoder
    /// output.
    ///
    /// `xa: None` is self-attention — K/V are always recomputed and `flush_cache`
    /// is irrelevant. `xa: Some(..)` is cross-attention, where K/V are cached on
    /// first use; `flush_cache` drops that cache so a new audio window is not
    /// decoded against the previous window's encoder state.
    fn forward(
        &mut self,
        x: &Tensor,
        xa: Option<&Tensor>,
        mask: Option<&Tensor>,
        flush_cache: bool,
    ) -> Result<Tensor> {
        let _enter = self.span.enter();
        let q = self.query.forward(x)?;
        let (k, v) = match xa {
            None => {
                let k = self.key.forward(x)?;
                let v = self.value.forward(x)?;
                (k, v)
            }
            Some(x) => {
                if flush_cache {
                    self.kv_cache = None;
                }
                if let Some((k, v)) = &self.kv_cache {
                    (k.clone(), v.clone())
                } else {
                    let k = self.key.forward(x)?;
                    let v = self.value.forward(x)?;
                    self.kv_cache = Some((k.clone(), v.clone()));
                    (k, v)
                }
            }
        };
        let wv = self.qkv_attention(&q, &k, &v, mask)?;
        let out = self.out.forward(&wv)?;
        Ok(out)
    }

    /// Split the model dimension into `n_head` heads: `(B, T, D)` → `(B, H, T, D/H)`.
    fn reshape_head(&self, x: &Tensor) -> Result<Tensor> {
        let (n_batch, n_ctx, n_state) = x.dims3()?;
        let target_dims = &[n_batch, n_ctx, self.n_head, n_state / self.n_head];
        x.reshape(target_dims)?.transpose(1, 2)
    }

    /// Scaled dot-product attention with an optional causal `mask`.
    ///
    /// The `(D/H)^-0.25` scale is folded into *both* Q and K before the matmul
    /// rather than applied once afterwards — the product is the same `1/sqrt(D/H)`
    /// but the intermediate magnitudes stay smaller, which matters in reduced
    /// precision.
    fn qkv_attention(
        &self,
        q: &Tensor,
        k: &Tensor,
        v: &Tensor,
        mask: Option<&Tensor>,
    ) -> Result<Tensor> {
        let (_, n_ctx, n_state) = q.dims3()?;
        let scale = ((n_state / self.n_head) as f64).powf(-0.25);
        let q = (self.reshape_head(q)? * scale)?;
        let k = (self.reshape_head(k)?.transpose(2, 3)? * scale)?;
        let v = self.reshape_head(v)?.contiguous()?;
        let mut qk = {
            let _enter = self.matmul_span.enter();
            q.matmul(&k)?
        };
        if let Some(mask) = mask {
            let mask = mask.i((0..n_ctx, 0..n_ctx))?;
            qk = qk.broadcast_add(&mask)?
        }
        let w = {
            let _enter = self.softmax_span.enter();
            candle_nn::ops::softmax_last_dim(&qk)?
        };
        let wv = {
            let _enter = self.matmul_span.enter();
            w.matmul(&v)?
        }
        .transpose(1, 2)?
        .flatten_from(2)?;
        Ok(wv)
    }

    /// Drop the cached cross-attention K/V.
    fn reset_kv_cache(&mut self) {
        self.kv_cache = None;
    }
}

/// One transformer block: pre-norm self-attention, optional cross-attention,
/// then a GELU MLP — each stage added residually.
///
/// `cross_attn` is `Some` only in decoder blocks; encoder blocks are built with
/// `ca: false` and never see encoder output.
// https://github.com/openai/whisper/blob/f572f2161ba831bae131364c3bffdead7af6d210/whisper/model.py#L111
#[derive(Debug, Clone)]
struct ResidualAttentionBlock {
    attn: MultiHeadAttention,
    attn_ln: LayerNorm,
    cross_attn: Option<(MultiHeadAttention, LayerNorm)>,
    mlp_linear1: Linear,
    mlp_linear2: Linear,
    mlp_ln: LayerNorm,
    span: tracing::Span,
}

impl ResidualAttentionBlock {
    /// Load one block. `ca` requests the cross-attention sub-layer (decoder only);
    /// the MLP hidden size is Whisper's fixed `4 * n_state`.
    fn load(n_state: usize, n_head: usize, ca: bool, vb: VarBuilder) -> Result<Self> {
        let span = tracing::span!(tracing::Level::TRACE, "residual-attn");
        let attn = MultiHeadAttention::load(n_state, n_head, vb.pp("self_attn"))?;
        let attn_ln = layer_norm(n_state, vb.pp("self_attn_layer_norm"))?;
        let cross_attn = if ca {
            let cross_attn = MultiHeadAttention::load(n_state, n_head, vb.pp("encoder_attn"))?;
            let cross_attn_ln = layer_norm(n_state, vb.pp("encoder_attn_layer_norm"))?;
            Some((cross_attn, cross_attn_ln))
        } else {
            None
        };
        let n_mlp = n_state * 4;
        let mlp_linear1 = linear(n_state, n_mlp, vb.pp("fc1"))?;
        let mlp_linear2 = linear(n_mlp, n_state, vb.pp("fc2"))?;
        let mlp_ln = layer_norm(n_state, vb.pp("final_layer_norm"))?;
        Ok(Self {
            attn,
            attn_ln,
            cross_attn,
            mlp_linear1,
            mlp_linear2,
            mlp_ln,
            span,
        })
    }

    /// Run the block. `xa` is the encoder output (ignored when this block has no
    /// cross-attention); `flush_kv_cache` is forwarded to the cross-attention
    /// module so a new audio window starts from a clean cache.
    fn forward(
        &mut self,
        x: &Tensor,
        xa: Option<&Tensor>,
        mask: Option<&Tensor>,
        flush_kv_cache: bool,
    ) -> Result<Tensor> {
        let _enter = self.span.enter();
        let attn = self
            .attn
            .forward(&self.attn_ln.forward(x)?, None, mask, flush_kv_cache)?;
        let mut x = (x + attn)?;
        if let Some((attn, ln)) = &mut self.cross_attn {
            x = (&x + attn.forward(&ln.forward(&x)?, xa, None, flush_kv_cache)?)?;
        }
        let mlp = self.mlp_linear2.forward(
            &self
                .mlp_linear1
                .forward(&self.mlp_ln.forward(&x)?)?
                .gelu()?,
        )?;
        x + mlp
    }

    /// Clear both attention caches in this block.
    fn reset_kv_cache(&mut self) {
        self.attn.reset_kv_cache();
        if let Some((attn, _)) = &mut self.cross_attn {
            attn.reset_kv_cache();
        }
    }
}

/// Build Whisper's fixed sinusoidal positional table, `(length, channels)`.
///
/// The encoder's positions are not learned weights — they are regenerated here at
/// load time, which is why no `embed_positions` tensor is read for the encoder
/// (the decoder, by contrast, does load a trained table).
fn sinusoids(length: usize, channels: usize, device: &Device) -> Result<Tensor> {
    let max_timescale = 10000f32;
    let log_timescale_increment = max_timescale.ln() / (channels / 2 - 1) as f32;
    let inv_timescales: Vec<_> = (0..channels / 2)
        .map(|i| (i as f32 * (-log_timescale_increment)).exp())
        .collect();
    let inv_timescales = Tensor::new(inv_timescales.as_slice(), device)?.unsqueeze(0)?;
    let arange = Tensor::arange(0, length as u32, device)?
        .to_dtype(DType::F32)?
        .unsqueeze(1)?;
    let sh = (length, channels / 2);
    let scaled_time = (arange.broadcast_as(sh)? * inv_timescales.broadcast_as(sh)?)?;
    let sincos = Tensor::cat(&[scaled_time.sin()?, scaled_time.cos()?], 1)?;
    Ok(sincos)
}

/// Audio encoder: two strided convolutions over the mel spectrogram, then a
/// stack of self-attention blocks.
///
/// `conv2` has stride 2, so the encoder emits one frame per two mel frames —
/// the 2× downsampling that makes the 30-second window fit `max_source_positions`.
// https://github.com/openai/whisper/blob/f572f2161ba831bae131364c3bffdead7af6d210/whisper/model.py#L143
#[derive(Debug, Clone)]
pub struct AudioEncoder {
    conv1: Conv1d,
    conv2: Conv1d,
    positional_embedding: Tensor,
    blocks: Vec<ResidualAttentionBlock>,
    ln_post: LayerNorm,
    span: tracing::Span,
    conv1_span: tracing::Span,
    conv2_span: tracing::Span,
}

impl AudioEncoder {
    /// Load the convolutions, the generated positional table, and
    /// `cfg.encoder_layers` cross-attention-free blocks.
    pub fn load(vb: VarBuilder, cfg: &Config) -> Result<Self> {
        let span = tracing::span!(tracing::Level::TRACE, "audio-encoder");
        let conv1_span = tracing::span!(tracing::Level::TRACE, "conv1");
        let conv2_span = tracing::span!(tracing::Level::TRACE, "conv2");
        let n_state = cfg.d_model;
        let n_head = cfg.encoder_attention_heads;
        let n_ctx = cfg.max_source_positions;
        let cfg1 = Conv1dConfig {
            padding: 1,
            stride: 1,
            groups: 1,
            dilation: 1,
            cudnn_fwd_algo: None,
        };
        let cfg2 = Conv1dConfig {
            padding: 1,
            stride: 2,
            groups: 1,
            dilation: 1,
            cudnn_fwd_algo: None,
        };
        let conv1 = conv1d(cfg.num_mel_bins, n_state, 3, cfg1, vb.pp("conv1"))?;
        let conv2 = conv1d(n_state, n_state, 3, cfg2, vb.pp("conv2"))?;
        let positional_embedding = sinusoids(n_ctx, n_state, vb.device())?;
        let blocks = (0..cfg.encoder_layers)
            .map(|i| {
                ResidualAttentionBlock::load(n_state, n_head, false, vb.pp(format!("layers.{i}")))
            })
            .collect::<Result<Vec<_>>>()?;
        let ln_post = layer_norm(n_state, vb.pp("layer_norm"))?;
        Ok(Self {
            conv1,
            conv2,
            positional_embedding,
            blocks,
            ln_post,
            conv1_span,
            conv2_span,
            span,
        })
    }

    /// Encode a mel spectrogram into the audio embedding the decoder cross-attends
    /// to.
    ///
    /// Sequences longer than the positional table are truncated rather than
    /// rejected: `seq_len` is clamped to `positional_embedding.dim(0)` and the
    /// input narrowed to match, so an over-long mel window loses its tail instead
    /// of failing the broadcast add.
    pub fn forward(&mut self, x: &Tensor, flush_kv_cache: bool) -> Result<Tensor> {
        let _enter = self.span.enter();
        let x = {
            let _enter = self.conv1_span.enter();
            self.conv1.forward(x)?.gelu()?
        };
        let x = {
            let _enter = self.conv2_span.enter();
            self.conv2.forward(&x)?.gelu()?
        };
        let x = x.transpose(1, 2)?;
        let (_bsize, seq_len, _hidden) = x.dims3()?;
        let max_pos = self.positional_embedding.dim(0)?;
        let seq_len = seq_len.min(max_pos);
        let x = x.narrow(1, 0, seq_len)?;
        let positional_embedding = self.positional_embedding.narrow(0, 0, seq_len)?;
        let mut x = x.broadcast_add(&positional_embedding)?;
        for block in self.blocks.iter_mut() {
            x = block.forward(&x, None, None, flush_kv_cache)?
        }
        let x = self.ln_post.forward(&x)?;
        Ok(x)
    }
}

/// Autoregressive text decoder over the encoder's audio embedding.
///
/// `mask` is the causal mask, materialised once at load time for the full
/// `max_target_positions` context and sliced per step. Unlike the encoder, the
/// positional table here is a **trained** tensor read from the checkpoint.
// https://github.com/openai/whisper/blob/f572f2161ba831bae131364c3bffdead7af6d210/whisper/model.py#L176
#[derive(Debug, Clone)]
pub struct TextDecoder {
    token_embedding: Embedding,
    positional_embedding: Tensor,
    blocks: Vec<ResidualAttentionBlock>,
    ln: LayerNorm,
    mask: Tensor,
    span: tracing::Span,
    span_final: tracing::Span,
}

impl TextDecoder {
    /// Load the token/positional embeddings, `cfg.decoder_layers` cross-attending
    /// blocks, and precompute the causal mask.
    pub fn load(vb: VarBuilder, cfg: &Config) -> Result<Self> {
        let span = tracing::span!(tracing::Level::TRACE, "text-decoder");
        let span_final = tracing::span!(tracing::Level::TRACE, "text-decoder-final");
        let n_state = cfg.d_model;
        let n_head = cfg.decoder_attention_heads;
        let n_ctx = cfg.max_target_positions;
        let token_embedding = embedding(cfg.vocab_size, n_state, vb.pp("embed_tokens"))?;
        let positional_embedding = vb.get((n_ctx, n_state), "embed_positions.weight")?;
        let blocks = (0..cfg.decoder_layers)
            .map(|i| {
                ResidualAttentionBlock::load(n_state, n_head, true, vb.pp(format!("layers.{i}")))
            })
            .collect::<Result<Vec<_>>>()?;
        let ln = layer_norm(n_state, vb.pp("layer_norm"))?;
        let mask: Vec<_> = (0..n_ctx)
            .flat_map(|i| (0..n_ctx).map(move |j| if j > i { f32::NEG_INFINITY } else { 0f32 }))
            .collect();
        let mask = Tensor::from_vec(mask, (n_ctx, n_ctx), vb.device())?;
        Ok(Self {
            token_embedding,
            positional_embedding,
            blocks,
            ln,
            mask,
            span,
            span_final,
        })
    }

    /// Run the token prefix `x` against the encoder output `xa`, returning hidden
    /// states — call [`TextDecoder::final_linear`] to turn them into logits.
    pub fn forward(&mut self, x: &Tensor, xa: &Tensor, flush_kv_cache: bool) -> Result<Tensor> {
        let _enter = self.span.enter();
        let last = x.dim(D::Minus1)?;
        let token_embedding = self.token_embedding.forward(x)?;
        let positional_embedding = self.positional_embedding.narrow(0, 0, last)?;
        let mut x = token_embedding.broadcast_add(&positional_embedding)?;
        for block in self.blocks.iter_mut() {
            x = block.forward(&x, Some(xa), Some(&self.mask), flush_kv_cache)?;
        }
        self.ln.forward(&x)
    }

    /// Project hidden states to vocabulary logits.
    ///
    /// Whisper ties the output projection to the token embedding matrix, so this
    /// reuses those weights transposed instead of holding a separate head.
    pub fn final_linear(&self, x: &Tensor) -> Result<Tensor> {
        let b_size = x.dim(0)?;
        let w = self.token_embedding.embeddings().broadcast_left(b_size)?;
        let logits = {
            let _enter = self.span_final.enter();
            x.matmul(&w.t()?)?
        };
        Ok(logits)
    }

    /// Clear every decoder block's attention cache.
    pub fn reset_kv_cache(&mut self) {
        for block in self.blocks.iter_mut() {
            block.reset_kv_cache();
        }
    }
}

/// The full model: audio encoder plus text decoder.
///
/// Both halves are public because the streaming path encodes an audio window
/// once and then drives the decoder across many steps against that one embedding.
// https://github.com/openai/whisper/blob/f572f2161ba831bae131364c3bffdead7af6d210/whisper/model.py#L221
#[derive(Debug, Clone)]
pub struct Whisper {
    pub encoder: AudioEncoder,
    pub decoder: TextDecoder,
}

impl Whisper {
    /// Load both halves from a checkpoint rooted at `model.encoder` / `model.decoder`.
    pub fn load(vb: &VarBuilder, config: Config) -> Result<Self> {
        let encoder = AudioEncoder::load(vb.pp("model.encoder"), &config)?;
        let decoder = TextDecoder::load(vb.pp("model.decoder"), &config)?;
        Ok(Self { encoder, decoder })
    }

    /// Clear every attention cache in the model — the reset the streaming path
    /// performs before decoding a fresh audio window.
    pub fn reset_kv_cache(&mut self) {
        self.encoder
            .blocks
            .iter_mut()
            .for_each(|b| b.reset_kv_cache());
        self.decoder.reset_kv_cache();
    }
}
