use clicky_core::sys::eapp::{
    decode_fixed_16_16, first_frame, register, stack_word, texture_upload_candidates,
    words_from_snapshot, GlImportRecord, GlTraceFixture,
};

fn load_fixture() -> GlTraceFixture {
    serde_json::from_str(include_str!("fixtures/eapp/tetris_gl_trace.json"))
        .expect("valid trace fixture json")
}

fn load_ea_logo_rgba5551() -> Vec<u8> {
    include_bytes!("fixtures/eapp/eaLogo_5551_50x50_rgba5551.bin").to_vec()
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn words_as_positions_xyzw(words: &[u32]) -> Vec<(f32, f32, f32, f32)> {
    words
        .chunks_exact(4)
        .map(|chunk| {
            (
                decode_fixed_16_16(chunk[0]),
                decode_fixed_16_16(chunk[1]),
                decode_fixed_16_16(chunk[2]),
                decode_fixed_16_16(chunk[3]),
            )
        })
        .collect()
}

fn words_as_pairs(words: &[u32]) -> Vec<(f32, f32)> {
    words
        .chunks_exact(2)
        .map(|chunk| (decode_fixed_16_16(chunk[0]), decode_fixed_16_16(chunk[1])))
        .collect()
}

fn decode_rgba5551(raw: &[u8], width: usize, height: usize) -> Vec<u32> {
    assert_eq!(raw.len(), width * height * 2);
    raw.chunks_exact(2)
        .map(|chunk| {
            let px = u16::from_le_bytes([chunk[0], chunk[1]]);
            let r = ((px >> 11) & 0x1f) as u32 * 255 / 31;
            let g = ((px >> 6) & 0x1f) as u32 * 255 / 31;
            let b = ((px >> 1) & 0x1f) as u32 * 255 / 31;
            let a = if (px & 0x1) != 0 { 255 } else { 0 };
            (a << 24) | (r << 16) | (g << 8) | b
        })
        .collect()
}

fn edge(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (px - ax) * (by - ay) - (py - ay) * (bx - ax)
}

fn rasterize_triangle(
    fb: &mut [u32],
    fb_width: usize,
    fb_height: usize,
    tex: &[u32],
    tex_width: usize,
    tex_height: usize,
    verts: &[(f32, f32, f32, f32); 3],
) {
    let min_x = verts
        .iter()
        .map(|v| v.0)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as i32;
    let min_y = verts
        .iter()
        .map(|v| v.1)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as i32;
    let max_x = verts
        .iter()
        .map(|v| v.0)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min((fb_width - 1) as f32) as i32;
    let max_y = verts
        .iter()
        .map(|v| v.1)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min((fb_height - 1) as f32) as i32;

    let area = edge(
        verts[0].0, verts[0].1, verts[1].0, verts[1].1, verts[2].0, verts[2].1,
    );
    if area == 0.0 {
        return;
    }

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let w0 = edge(verts[1].0, verts[1].1, verts[2].0, verts[2].1, px, py) / area;
            let w1 = edge(verts[2].0, verts[2].1, verts[0].0, verts[0].1, px, py) / area;
            let w2 = edge(verts[0].0, verts[0].1, verts[1].0, verts[1].1, px, py) / area;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let u = verts[0].2 * w0 + verts[1].2 * w1 + verts[2].2 * w2;
            let v = verts[0].3 * w0 + verts[1].3 * w1 + verts[2].3 * w2;
            let tx = u.floor().clamp(0.0, (tex_width - 1) as f32) as usize;
            let ty = v.floor().clamp(0.0, (tex_height - 1) as f32) as usize;
            let color = tex[ty * tex_width + tx];
            fb[y as usize * fb_width + x as usize] = color;
        }
    }
}

fn render_quad(
    tex: &[u32],
    tex_width: usize,
    tex_height: usize,
    positions: &[(f32, f32)],
    uvs: &[(f32, f32)],
) -> Vec<u32> {
    let mut fb = vec![0u32; 320 * 240];
    let tri0 = [
        (positions[0].0, positions[0].1, uvs[0].0, uvs[0].1),
        (positions[1].0, positions[1].1, uvs[1].0, uvs[1].1),
        (positions[2].0, positions[2].1, uvs[2].0, uvs[2].1),
    ];
    let tri1 = [
        (positions[0].0, positions[0].1, uvs[0].0, uvs[0].1),
        (positions[2].0, positions[2].1, uvs[2].0, uvs[2].1),
        (positions[3].0, positions[3].1, uvs[3].0, uvs[3].1),
    ];
    rasterize_triangle(&mut fb, 320, 240, tex, tex_width, tex_height, &tri0);
    rasterize_triangle(&mut fb, 320, 240, tex, tex_width, tex_height, &tri1);
    fb
}

fn write_ppm(path: &std::path::Path, fb: &[u32], width: usize, height: usize) {
    let mut out = Vec::with_capacity(width * height * 3 + 64);
    out.extend_from_slice(format!("P6\n{} {}\n255\n", width, height).as_bytes());
    for &px in fb {
        out.push(((px >> 16) & 0xff) as u8);
        out.push(((px >> 8) & 0xff) as u8);
        out.push((px & 0xff) as u8);
    }
    std::fs::write(path, out).expect("write ppm");
}

fn seq_record<'a>(
    frame: &'a clicky_core::sys::eapp::GlFrameRecord,
    seq: u64,
) -> &'a GlImportRecord {
    frame
        .records
        .iter()
        .find(|record| record.seq_in_frame == seq)
        .unwrap_or_else(|| panic!("missing seq_in_frame {}", seq))
}

#[test]
fn fixed_16_16_decodes_signed_values() {
    assert_eq!(decode_fixed_16_16(0x0001_0000), 1.0);
    assert_eq!(decode_fixed_16_16(0x0000_8000), 0.5);
    assert_eq!(decode_fixed_16_16(0xffff_8000), -0.5);
    assert_eq!(decode_fixed_16_16(0x00ef_8000), 239.5);
}

#[test]
fn decodes_real_texture_triplet_and_quad_arrays() {
    let fixture = load_fixture();
    let uploads = texture_upload_candidates(&fixture);
    let logo = uploads
        .iter()
        .find(|candidate| {
            candidate.width == 50
                && candidate.height == 50
                && candidate.pixel_type == 0x8034
                && candidate
                    .source_file
                    .as_ref()
                    .map(|file| file.path.as_str())
                    == Some("eaLogo_5551.pix")
        })
        .expect("50x50 eaLogo upload");
    assert_eq!(logo.source_ptr, 0x1001_45d6);
    assert_eq!(logo.source_file.as_ref().unwrap().offset, 70);

    let frame4 = first_frame(&fixture, 4).expect("steady-state frame");
    let tex_record = seq_record(frame4, 31);
    assert_eq!(register(tex_record, "r0").unwrap().value, 0x1b);

    let pos_record = seq_record(frame4, 32);
    let pos_snapshot = stack_word(pos_record, 0x04)
        .and_then(|word| word.snapshot.as_ref())
        .expect("position snapshot");
    let pos = words_as_positions_xyzw(&words_from_snapshot(pos_snapshot));
    assert_eq!(pos[0], (0.0, 0.0, 0.0, 1.0));
    assert_eq!(pos[1], (0.0, 50.0, 0.0, 1.0));
    assert_eq!(pos[2], (50.0, 50.0, 0.0, 1.0));
    assert_eq!(pos[3], (50.0, 0.0, 0.0, 1.0));

    let uv_record = seq_record(frame4, 35);
    let uv_snapshot = stack_word(uv_record, 0x04)
        .and_then(|word| word.snapshot.as_ref())
        .expect("uv snapshot");
    let uv = words_as_pairs(&words_from_snapshot(uv_snapshot));
    assert_eq!(uv[0], (0.5, 49.5));
    assert_eq!(uv[1], (0.5, -0.5));
    assert_eq!(uv[2], (50.5, -0.5));
    assert_eq!(uv[3], (50.5, 49.5));

    let t0 = seq_record(frame4, 29);
    let t1 = seq_record(frame4, 30);
    let tx = register(t0, "r1").unwrap().float_value.unwrap()
        + register(t1, "r1").unwrap().float_value.unwrap();
    let ty = register(t0, "r2").unwrap().float_value.unwrap()
        + register(t1, "r2").unwrap().float_value.unwrap();
    assert_eq!((tx, ty), (235.0, 79.0));
}

#[test]
fn renders_real_tetris_quad_and_hashes_framebuffer() {
    let fixture = load_fixture();
    let frame4 = first_frame(&fixture, 4).expect("steady-state frame");

    let pos_record = seq_record(frame4, 32);
    let pos_snapshot = stack_word(pos_record, 0x04)
        .and_then(|word| word.snapshot.as_ref())
        .expect("position snapshot");
    let pos_local = words_as_positions_xyzw(&words_from_snapshot(pos_snapshot));

    let uv_record = seq_record(frame4, 35);
    let uv_snapshot = stack_word(uv_record, 0x04)
        .and_then(|word| word.snapshot.as_ref())
        .expect("uv snapshot");
    let uv = words_as_pairs(&words_from_snapshot(uv_snapshot));

    let t0 = seq_record(frame4, 29);
    let t1 = seq_record(frame4, 30);
    let tx = register(t0, "r1").unwrap().float_value.unwrap()
        + register(t1, "r1").unwrap().float_value.unwrap();
    let ty = register(t0, "r2").unwrap().float_value.unwrap()
        + register(t1, "r2").unwrap().float_value.unwrap();

    let positions: Vec<(f32, f32)> = pos_local
        .iter()
        .map(|(x, y, _, _)| (x + tx, y + ty))
        .collect();

    let tex = decode_rgba5551(&load_ea_logo_rgba5551(), 50, 50);
    let fb = render_quad(&tex, 50, 50, &positions, &uv);

    let mut bytes = Vec::with_capacity(fb.len() * 4);
    for px in &fb {
        bytes.extend_from_slice(&px.to_le_bytes());
    }
    let hash = fnv1a64(&bytes);
    if std::env::var_os("CLICKY_WRITE_TETRIS_QUAD_PPM").is_some() {
        write_ppm(std::path::Path::new("/tmp/tetris_quad3.ppm"), &fb, 320, 240);
    }
    assert_eq!(hash, 0xc6913d1457cb5696);
}
