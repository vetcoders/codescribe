use anyhow::Result;
use candle_core::safetensors::MmapedSafetensors;
use std::path::PathBuf;

fn main() -> Result<()> {
    let model_path = PathBuf::from(
        "/Users/maciejgad/hosted/VetCoders/CodeScribe/models/whisper-large-v3-mlx-q8/weights.safetensors",
    );
    let tensors = unsafe { MmapedSafetensors::new(&model_path)? };
    let all_tensors = tensors.tensors();
    println!("Found {} tensors.", all_tensors.len());
    for (name, _) in all_tensors.iter().take(20) {
        println!("{}", name);
    }
    Ok(())
}
