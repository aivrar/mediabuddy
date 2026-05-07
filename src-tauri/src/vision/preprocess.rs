//! Image preprocessing for Florence-2.
//!
//! Florence-2's image processor (BartTokenizer-compatible)
//! resizes input to 768x768 and normalizes with ImageNet stats:
//!   mean = [0.485, 0.456, 0.406]
//!   std  = [0.229, 0.224, 0.225]
//!
//! Output tensor shape: `[1, 3, 768, 768]` in NCHW float32 order.

use image::imageops::FilterType;
use image::DynamicImage;
use ndarray::{Array4, ArrayView4};

use crate::error::{AppError, Result};

#[derive(Debug, Clone, Copy)]
pub struct ImageNetStats {
    pub mean: [f32; 3],
    pub std: [f32; 3],
}

impl Default for ImageNetStats {
    fn default() -> Self {
        Self {
            mean: [0.485, 0.456, 0.406],
            std: [0.229, 0.224, 0.225],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PreprocessConfig {
    pub size: u32, // 768 for Florence-2 base
    pub stats: ImageNetStats,
}

impl Default for PreprocessConfig {
    fn default() -> Self {
        Self {
            size: 768,
            stats: ImageNetStats::default(),
        }
    }
}

/// Load an image from disk, resize, normalize, return NCHW float32 tensor.
pub fn preprocess_image(path: &std::path::Path, cfg: PreprocessConfig) -> Result<Array4<f32>> {
    let img = image::open(path)?;
    let resized = img
        .resize_exact(cfg.size, cfg.size, FilterType::CatmullRom)
        .to_rgb8();

    let size = cfg.size as usize;
    let mut tensor = Array4::<f32>::zeros((1, 3, size, size));
    let stats = cfg.stats;

    for (y, row) in resized.rows().enumerate() {
        for (x, pixel) in row.enumerate() {
            let r = pixel.0[0] as f32 / 255.0;
            let g = pixel.0[1] as f32 / 255.0;
            let b = pixel.0[2] as f32 / 255.0;
            tensor[[0, 0, y, x]] = (r - stats.mean[0]) / stats.std[0];
            tensor[[0, 1, y, x]] = (g - stats.mean[1]) / stats.std[1];
            tensor[[0, 2, y, x]] = (b - stats.mean[2]) / stats.std[2];
        }
    }

    Ok(tensor)
}

/// Same preprocessing but from already-decoded image bytes.
#[allow(dead_code)]
pub fn preprocess_dynamic(img: DynamicImage, cfg: PreprocessConfig) -> Result<Array4<f32>> {
    let resized = img
        .resize_exact(cfg.size, cfg.size, FilterType::CatmullRom)
        .to_rgb8();
    let size = cfg.size as usize;
    let mut tensor = Array4::<f32>::zeros((1, 3, size, size));
    let stats = cfg.stats;
    for (y, row) in resized.rows().enumerate() {
        for (x, pixel) in row.enumerate() {
            let r = pixel.0[0] as f32 / 255.0;
            let g = pixel.0[1] as f32 / 255.0;
            let b = pixel.0[2] as f32 / 255.0;
            tensor[[0, 0, y, x]] = (r - stats.mean[0]) / stats.std[0];
            tensor[[0, 1, y, x]] = (g - stats.mean[1]) / stats.std[1];
            tensor[[0, 2, y, x]] = (b - stats.mean[2]) / stats.std[2];
        }
    }
    Ok(tensor)
}

#[allow(dead_code)]
pub fn assert_shape(view: &ArrayView4<f32>, size: usize) -> Result<()> {
    if view.shape() != [1, 3, size, size] {
        return Err(AppError::other(format!(
            "expected NCHW [1, 3, {size}, {size}], got {:?}",
            view.shape()
        )));
    }
    Ok(())
}
