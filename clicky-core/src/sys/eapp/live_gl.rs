//! Live OpenGLES HLE state for the experimental first-pass renderer.
//!
//! This module holds the *data* and *pure* helpers for the opt-in live GL HLE
//! path (`CLICKY_EXPERIMENTAL_GL_HLE=1`). All guest-memory access is performed
//! in `mod.rs` (where the bus lives); this module only reasons about decoded
//! state, texture selection, framebuffer presentation, and bounded diagnostics.
//!
//! Scope rules (see docs/EAPP_GL_TRACE_DECODER_REPORT.md):
//! - texture row order is preserved (no row inversion at decode time);
//! - captured UVs and guest geometry are preserved;
//! - the internal rasterizer framebuffer is kept in its native (unflipped) order;
//! - a vertical presentation flip is applied **only** when serializing/presenting;
//! - the flip is a diagnostic/presentation convenience, not a confirmed ABI rule.

use std::collections::{HashMap, HashSet};

use super::gl_decode::{format_from_gl, pix_payload_size};
use super::rasterizer::{
    framebuffer_hash, framebuffer_to_ppm, rasterize_quad, Rgba8, Texture, TextureFormat,
};

pub const FB_WIDTH: usize = 320;
pub const FB_HEIGHT: usize = 240;
pub const FB_PIXELS: usize = FB_WIDTH * FB_HEIGHT;

/// GL_FIXED (0x140c) enumerant confirmed by disassembly for the position/UV
/// arrays. Any other array format is preserved but not interpreted.
pub const GL_FIXED: u32 = 0x140c;

/// Confirmed DrawArrays mode token observed at every ordinal-37 call site.
pub const DRAW_MODE: u32 = 7;

/// A live texture upload captured at ordinal-99 call time. Pixel bytes are
/// copied immediately from guest memory; row order is preserved as uploaded.
#[derive(Debug, Clone)]
pub struct LiveGlUpload {
    pub index: usize,
    pub target: u32,
    pub width: usize,
    pub height: usize,
    pub source_format: u32,
    pub pixel_type: u32,
    pub source_ptr: u32,
    pub format: Option<TextureFormat>,
    pub texture: Option<Texture>,
}

/// A vertex array definition recorded from ordinal-137. Unknown slots are
/// preserved verbatim without assigning unsupported semantic names.
#[derive(Debug, Clone, Default)]
pub struct LiveArrayDef {
    pub array_index: u32,
    pub component_count: u32,
    pub format: u32,
    pub stride: u32,
    pub guest_ptr: u32,
    pub valid: bool,
}

/// One decoded ordinal-37 draw, recorded for diagnostics and comparison.
#[derive(Debug, Clone)]
pub struct LiveDrawRecord {
    pub draw_index: usize,
    pub handle: u32,
    pub state_ptr: u32,
    pub translation: (f32, f32),
    pub positions: [(f32, f32); 4],
    pub uvs: [(f32, f32); 4],
    pub has_uv: bool,
    pub bounds: (f32, f32, f32, f32),
    pub coverage: u64,
    pub selected_upload: Option<usize>,
    pub inferred_dim: Option<(usize, usize)>,
    pub skipped_reason: Option<String>,
}

/// Persistent per-eapp live graphics state, sufficient for the observed
/// Tetris stream. Stored on `Eapp` only when the experimental flag is set.
pub struct LiveGlState {
    pub uploads: Vec<LiveGlUpload>,
    pub arrays: HashMap<u32, LiveArrayDef>,
    pub enabled_arrays: HashSet<u32>,
    pub current_handle: u32,
    pub current_state_ptr: u32,
    pub translation: (f32, f32),
    pub framebuffer: Vec<Rgba8>,
    pub draws: Vec<LiveDrawRecord>,
    pub draw_count_in_frame: usize,
    pub candidate_frames: usize,
    pub captured_first_frame: bool,
    pub present_vflip: bool,
    pub gate_b: bool,
    pub continuous_capture: bool,
    pub last_frame_counter: u64,
    /// Draw-handle signature of the previous 4-draw frame, used to detect the
    /// steady-state frame (first consecutive repeat) for default-mode capture.
    pub prev_draw_handles: Option<Vec<u32>>,
    /// Tentative lifecycle observations around ordinals 157/158/165. We record
    /// the observed ordering but do not rename them present/begin/end.
    pub lifecycle_log: Vec<String>,
    /// Most recent presented framebuffer (post optional vflip), kept so Gate B
    /// can copy it to the desktop window independently of the internal buffer.
    pub presented: Option<Vec<Rgba8>>,
}

impl LiveGlState {
    pub fn new(present_vflip: bool, gate_b: bool, continuous_capture: bool) -> Self {
        Self {
            uploads: Vec::new(),
            arrays: HashMap::new(),
            enabled_arrays: HashSet::new(),
            current_handle: 0,
            current_state_ptr: 0,
            translation: (0.0, 0.0),
            framebuffer: vec![Rgba8::rgba(0, 0, 0, 0); FB_PIXELS],
            draws: Vec::new(),
            draw_count_in_frame: 0,
            candidate_frames: 0,
            captured_first_frame: false,
            present_vflip,
            gate_b,
            continuous_capture,
            last_frame_counter: 0,
            prev_draw_handles: None,
            lifecycle_log: Vec::new(),
            presented: None,
        }
    }

    /// Reset per-frame accumulators. Uploads persist (they happen once at
    /// startup); arrays/enabled are cleared because they are redefined each
    /// frame by ordinal-137/40 calls.
    pub fn reset_for_frame(&mut self) {
        self.arrays.clear();
        self.enabled_arrays.clear();
        self.translation = (0.0, 0.0);
        self.framebuffer = vec![Rgba8::rgba(0, 0, 0, 0); FB_PIXELS];
        self.draws.clear();
        self.draw_count_in_frame = 0;
    }

    /// Build a `LiveGlUpload` from decoded ordinal-99 arguments, copying the
    /// supplied guest pixel bytes immediately. Row order is preserved.
    pub fn build_upload(
        index: usize,
        target: u32,
        width: u32,
        height: u32,
        source_format: u32,
        pixel_type: u32,
        source_ptr: u32,
        payload: &[u8],
    ) -> LiveGlUpload {
        let format = format_from_gl(source_format, pixel_type);
        let texture = format.and_then(|fmt| {
            let expected = pix_payload_size(fmt, width as usize, height as usize);
            if payload.len() < expected {
                return None;
            }
            Some(Texture::from_bytes(
                &payload[..expected],
                width as usize,
                height as usize,
                fmt,
                // A8 tint: white, matching the offline replay convention.
                Rgba8::rgba(255, 255, 255, 255),
            ))
        });
        LiveGlUpload {
            index,
            target,
            width: width as usize,
            height: height as usize,
            source_format,
            pixel_type,
            source_ptr,
            format,
            texture,
        }
    }

    /// Select the best-supported live texture by matching decoded draw
    /// dimensions. This is an *inferred* association (logged as such); it
    /// prefers live upload evidence (dimensions/format) over filenames.
    pub fn select_upload_by_dims(&self, w: usize, h: usize) -> Option<usize> {
        self.uploads
            .iter()
            .find(|u| u.texture.is_some() && u.width == w && u.height == h)
            .map(|u| u.index)
    }

    /// Rasterize one draw into the internal framebuffer using the existing
    /// rasterizer. Returns the produced `LiveDrawRecord`.
    pub fn rasterize_draw(
        &mut self,
        draw_index: usize,
        handle: u32,
        state_ptr: u32,
        translation: (f32, f32),
        positions: [(f32, f32); 4],
        uvs: [(f32, f32); 4],
        has_uv: bool,
    ) -> LiveDrawRecord {
        let bounds = bounds_for(&positions);
        let inferred_dim = if has_uv {
            let (w, h) = infer_dims_from_uvs(&uvs);
            Some((w, h))
        } else {
            None
        };

        let selected_upload = inferred_dim.and_then(|(w, h)| self.select_upload_by_dims(w, h));

        let mut record = LiveDrawRecord {
            draw_index,
            handle,
            state_ptr,
            translation,
            positions,
            uvs,
            has_uv,
            bounds,
            coverage: 0,
            selected_upload,
            inferred_dim,
            skipped_reason: None,
        };

        let Some(upload_idx) = selected_upload else {
            record.skipped_reason = Some(format!(
                "no live upload matched inferred dims {:?} (handle={:#x})",
                inferred_dim, handle
            ));
            return record;
        };
        let Some(texture) = self.uploads.get(upload_idx).and_then(|u| u.texture.clone()) else {
            record.skipped_reason =
                Some(format!("upload #{upload_idx} has no decoded texture"));
            return record;
        };

        record.coverage = rasterize_quad(
            &mut self.framebuffer,
            FB_WIDTH,
            FB_HEIGHT,
            &texture,
            &positions,
            &uvs,
        );
        record
    }

    /// Produce the presented framebuffer (a copy), applying the configurable
    /// vertical presentation flip when enabled. The internal framebuffer is
    /// never mutated by presentation.
    pub fn present(&self) -> Vec<Rgba8> {
        let mut out = self.framebuffer.clone();
        if self.present_vflip {
            flip_vertical_in_place(&mut out, FB_WIDTH, FB_HEIGHT);
        }
        out
    }

    pub fn internal_hash(&self) -> u64 {
        framebuffer_hash(&self.framebuffer)
    }

    pub fn presented_hash(&self) -> u64 {
        let presented = self.present();
        framebuffer_hash(&presented)
    }

    /// Write both diagnostic PPMs (internal = native order, presented = with
    /// optional vflip). Returns true if both writes succeeded.
    pub fn write_diagnostic_ppms(
        &self,
        internal_path: &std::path::Path,
        presented_path: &std::path::Path,
    ) -> bool {
        let presented = self.present();
        let ok_a = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            framebuffer_to_ppm(internal_path, &self.framebuffer, FB_WIDTH, FB_HEIGHT);
        }))
        .is_ok();
        let ok_b = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            framebuffer_to_ppm(presented_path, &presented, FB_WIDTH, FB_HEIGHT);
        }))
        .is_ok();
        ok_a && ok_b
    }
}

fn bounds_for(positions: &[(f32, f32); 4]) -> (f32, f32, f32, f32) {
    positions.iter().fold(
        (
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ),
        |acc, (x, y)| (acc.0.min(*x), acc.1.min(*y), acc.2.max(*x), acc.3.max(*y)),
    )
}

/// Infer intended texture dimensions from texel-centered UVs. The captured
/// UVs are half-texel centered (e.g. 0.5 .. 50.5 for a 50px texture), so the
/// span rounds to the texture dimension.
fn infer_dims_from_uvs(uvs: &[(f32, f32); 4]) -> (usize, usize) {
    let (min_u, min_v, max_u, max_v) = uvs.iter().fold(
        (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY),
        |acc, (u, v)| (acc.0.min(*u), acc.1.min(*v), acc.2.max(*u), acc.3.max(*v)),
    );
    let w = (max_u - min_u).round().max(1.0) as usize;
    let h = (max_v - min_v).round().max(1.0) as usize;
    (w, h)
}

/// Flip a framebuffer vertically in place. Used only for presentation output.
pub fn flip_vertical_in_place(fb: &mut [Rgba8], width: usize, height: usize) {
    for y in 0..(height / 2) {
        let top = y * width;
        let bottom = (height - 1 - y) * width;
        for col in 0..width {
            fb.swap(top + col, bottom + col);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb565_2x2() -> Vec<u8> {
        // 4 pixels, 2 bytes each, all distinct so flips are detectable.
        vec![0x00, 0xf8, 0xe0, 0x07, 0x1f, 0x00, 0xff, 0xff]
    }

    #[test]
    fn build_upload_decodes_pixels_and_preserves_dims() {
        let payload = rgb565_2x2();
        let upload = LiveGlState::build_upload(
            0,
            0x0de1,
            2,
            2,
            0x1907,
            0x8363,
            0x1000_0000,
            &payload,
        );
        assert_eq!(upload.format, Some(TextureFormat::Rgb565));
        assert_eq!(upload.width, 2);
        assert_eq!(upload.height, 2);
        let tex = upload.texture.expect("texture decoded");
        assert_eq!(tex.width, 2);
        assert_eq!(tex.height, 2);
        assert_eq!(tex.pixels.len(), 4);
        // top-left pixel is red in the source 565 layout
        assert_eq!(tex.pixels[0].r, 255);
    }

    #[test]
    fn build_upload_rejects_short_payload() {
        let payload = vec![0u8; 2]; // far too short for 2x2 RGB565
        let upload =
            LiveGlState::build_upload(0, 0x0de1, 2, 2, 0x1907, 0x8363, 0x1000_0000, &payload);
        assert_eq!(upload.format, Some(TextureFormat::Rgb565));
        assert!(upload.texture.is_none(), "short payload must not decode");
    }

    #[test]
    fn build_upload_rejects_unsupported_format() {
        let upload = LiveGlState::build_upload(0, 0x0de1, 2, 2, 0xdead, 0xbeef, 0x1000_0000, &[]);
        assert!(upload.format.is_none());
        assert!(upload.texture.is_none());
    }

    #[test]
    fn select_upload_matches_by_dimensions() {
        let mut lg = LiveGlState::new(true, false, false);
        lg.uploads.push(LiveGlState::build_upload(
            0,
            0x0de1,
            50,
            50,
            0x1908,
            0x8034,
            0x1000_0000,
            &vec![0u8; 50 * 50 * 2],
        ));
        lg.uploads.push(LiveGlState::build_upload(
            1,
            0x0de1,
            250,
            162,
            0x1908,
            0x8033,
            0x1000_0010,
            &vec![0u8; 250 * 162 * 2],
        ));
        assert_eq!(lg.select_upload_by_dims(50, 50), Some(0));
        assert_eq!(lg.select_upload_by_dims(250, 162), Some(1));
        assert_eq!(lg.select_upload_by_dims(999, 999), None);
    }

    #[test]
    fn present_applies_configurable_vflip_only_when_enabled() {
        let mut lg = LiveGlState::new(false, false, false);
        lg.framebuffer[0] = Rgba8::rgba(255, 0, 0, 255);
        lg.framebuffer[FB_WIDTH * (FB_HEIGHT - 1)] = Rgba8::rgba(0, 0, 255, 255);
        let no_flip = lg.present();
        assert_eq!(no_flip[0], Rgba8::rgba(255, 0, 0, 255));
        assert_eq!(no_flip[FB_WIDTH * (FB_HEIGHT - 1)], Rgba8::rgba(0, 0, 255, 255));

        lg.present_vflip = true;
        let flipped = lg.present();
        assert_eq!(flipped[0], Rgba8::rgba(0, 0, 255, 255));
        assert_eq!(flipped[FB_WIDTH * (FB_HEIGHT - 1)], Rgba8::rgba(255, 0, 0, 255));
        // internal buffer is never mutated by presentation
        assert_eq!(lg.framebuffer[0], Rgba8::rgba(255, 0, 0, 255));
    }

    #[test]
    fn infer_dims_from_texel_centered_uvs() {
        // 50x50 texture: UVs span 0.5..50.5 in both axes
        let uvs = [
            (0.5, 0.5),
            (0.5, -0.5),
            (50.5, -0.5),
            (50.5, 49.5),
        ];
        let (w, h) = super::infer_dims_from_uvs(&uvs);
        assert_eq!((w, h), (50, 50));
    }

    #[test]
    fn reset_for_frame_clears_per_frame_state_but_keeps_uploads() {
        let mut lg = LiveGlState::new(true, false, false);
        lg.uploads.push(LiveGlState::build_upload(
            0,
            0x0de1,
            2,
            2,
            0x1907,
            0x8363,
            0x1000_0000,
            &rgb565_2x2(),
        ));
        lg.translation = (10.0, 20.0);
        lg.draw_count_in_frame = 2;
        lg.framebuffer[0] = Rgba8::rgba(1, 2, 3, 4);
        lg.reset_for_frame();
        assert_eq!(lg.translation, (0.0, 0.0));
        assert_eq!(lg.draw_count_in_frame, 0);
        assert_eq!(lg.framebuffer[0], Rgba8::rgba(0, 0, 0, 0));
        assert_eq!(lg.uploads.len(), 1, "uploads persist across frames");
    }
}
