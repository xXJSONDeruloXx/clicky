use clicky_core::sys::eapp::{
    blend_src_over, decode_fixed_16_16, first_frame, framebuffer_hash, framebuffer_to_ppm,
    rasterize_quad, register, sample_nearest, stack_word, texture_upload_candidates,
    words_from_snapshot, GlImportRecord, GlTraceFixture, Rgba8, Texture, TextureFormat,
};

fn load_fixture() -> GlTraceFixture {
    serde_json::from_str(include_str!("fixtures/eapp/tetris_gl_trace.json"))
        .expect("valid trace fixture json")
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

fn make_raw_rgb565(width: usize, height: usize) -> Vec<u8> {
    let mut raw = Vec::with_capacity(width * height * 2);
    for y in 0..height {
        for x in 0..width {
            let r = ((x as u16) * 31 / (width as u16 - 1)) & 0x1f;
            let g = ((y as u16) * 63 / (height as u16 - 1)) & 0x3f;
            let b = (((x + y) as u16) * 31 / ((width + height - 2) as u16)) & 0x1f;
            let px = (r << 11) | (g << 5) | b;
            raw.extend_from_slice(&px.to_le_bytes());
        }
    }
    raw
}

fn make_raw_rgba5551(width: usize, height: usize) -> Vec<u8> {
    let mut raw = Vec::with_capacity(width * height * 2);
    for y in 0..height {
        for x in 0..width {
            let r = ((x as u16) * 31 / (width as u16 - 1)) & 0x1f;
            let g = ((y as u16) * 31 / (height as u16 - 1)) & 0x1f;
            let b = (((x + y) as u16) * 31 / ((width + height - 2) as u16)) & 0x1f;
            let a = if ((x / 4) + (y / 4)) % 2 == 0 { 1 } else { 0 };
            let px = (r << 11) | (g << 6) | (b << 1) | a;
            raw.extend_from_slice(&px.to_le_bytes());
        }
    }
    raw
}

fn make_raw_rgba4444(width: usize, height: usize) -> Vec<u8> {
    let mut raw = Vec::with_capacity(width * height * 2);
    for y in 0..height {
        for x in 0..width {
            let r = ((x as u16) * 15 / (width as u16 - 1)) & 0x0f;
            let g = ((y as u16) * 15 / (height as u16 - 1)) & 0x0f;
            let b = (((x + y) as u16) * 15 / ((width + height - 2) as u16)) & 0x0f;
            let a = (((x ^ y) as u16) * 15 / ((width.max(height) - 1) as u16)) & 0x0f;
            let px = (r << 12) | (g << 8) | (b << 4) | a;
            raw.extend_from_slice(&px.to_le_bytes());
        }
    }
    raw
}

fn make_raw_a8(width: usize, height: usize) -> Vec<u8> {
    let mut raw = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let alpha = if width == 1 && height == 1 {
                128
            } else {
                ((x * 255 / (width - 1)) ^ (y * 255 / (height - 1))) as u8
            };
            raw.push(alpha);
        }
    }
    raw
}

fn make_texture(format: TextureFormat, width: usize, height: usize, raw: Vec<u8>) -> Texture {
    Texture::from_bytes(&raw, width, height, format, Rgba8::rgba(255, 255, 255, 255))
}

fn replay_frame4() -> (Vec<Rgba8>, Vec<DrawReplay>) {
    let fixture = load_fixture();
    let frame4 = first_frame(&fixture, 4).expect("steady-state frame");

    let draws = vec![
        DrawReplay::from_frame(
            frame4,
            DrawPlan {
                seqs_169: &[3, 4, 5],
                seq_159: 6,
                seq_pos: 7,
                seq_uv: 10,
                seq_aux: Some(11),
                proposed_texture: ProposedTexture::resolved(
                    "screenBG_565.pix",
                    320,
                    240,
                    TextureFormat::Rgb565,
                    0.93,
                ),
                texture: make_texture(TextureFormat::Rgb565, 320, 240, make_raw_rgb565(320, 240)),
            },
        ),
        DrawReplay::from_frame(
            frame4,
            DrawPlan {
                seqs_169: &[18, 19],
                seq_159: 20,
                seq_pos: 21,
                seq_uv: 24,
                seq_aux: None,
                proposed_texture: ProposedTexture::resolved(
                    "tetrisLogo_4444.pix",
                    250,
                    162,
                    TextureFormat::Rgba4444,
                    0.84,
                ),
                texture: make_texture(
                    TextureFormat::Rgba4444,
                    250,
                    162,
                    make_raw_rgba4444(250, 162),
                ),
            },
        ),
        DrawReplay::from_frame(
            frame4,
            DrawPlan {
                seqs_169: &[29, 30],
                seq_159: 31,
                seq_pos: 32,
                seq_uv: 35,
                seq_aux: None,
                proposed_texture: ProposedTexture::resolved(
                    "eaLogo_5551.pix",
                    50,
                    50,
                    TextureFormat::Rgba5551,
                    0.87,
                ),
                texture: make_texture(TextureFormat::Rgba5551, 50, 50, make_raw_rgba5551(50, 50)),
            },
        ),
        DrawReplay::from_frame(
            frame4,
            DrawPlan {
                seqs_169: &[40],
                seq_159: 41,
                seq_pos: 42,
                seq_uv: 44,
                seq_aux: None,
                proposed_texture: ProposedTexture::unresolved(
                    "generated placeholder",
                    "handle 3 / full-screen overlay",
                    TextureFormat::A8,
                    0.28,
                ),
                texture: make_texture(TextureFormat::A8, 1, 1, make_raw_a8(1, 1)),
            },
        ),
    ];

    let mut fb = vec![Rgba8::rgba(0, 0, 0, 0); 320 * 240];
    for draw in &draws {
        draw.rasterize(&mut fb);
    }
    (fb, draws)
}

#[derive(Debug, Clone)]
struct DrawPlan {
    seqs_169: &'static [u64],
    seq_159: u64,
    seq_pos: u64,
    seq_uv: u64,
    seq_aux: Option<u64>,
    proposed_texture: ProposedTexture,
    texture: Texture,
}

#[derive(Debug, Clone)]
struct DrawReplay {
    ordinal159_handle: u32,
    state_ptr: u32,
    translation: (f32, f32),
    local_positions: [(f32, f32); 4],
    uv_or_aux: Vec<(f32, f32)>,
    aux_array: Option<Vec<(f32, f32)>>,
    screen_bounds: (f32, f32, f32, f32),
    proposed_texture: ProposedTexture,
    texture: Texture,
    coverage: u64,
}

impl DrawReplay {
    fn from_frame(frame: &clicky_core::sys::eapp::GlFrameRecord, plan: DrawPlan) -> Self {
        let mut tx = 0.0f32;
        let mut ty = 0.0f32;
        for seq in plan.seqs_169 {
            let record = seq_record(frame, *seq);
            tx += f32::from_bits(register(record, "r1").unwrap().value);
            ty += f32::from_bits(register(record, "r2").unwrap().value);
        }

        let record_159 = seq_record(frame, plan.seq_159);
        let ordinal159_handle = register(record_159, "r0").unwrap().value;
        let state_ptr = register(record_159, "r1").unwrap().value;

        let pos_words = stack_word(seq_record(frame, plan.seq_pos), 0x04)
            .and_then(|word| word.snapshot.as_ref())
            .expect("position snapshot");
        let local_positions = {
            let points = words_as_positions_xyzw(&words_from_snapshot(pos_words));
            [
                (points[0].0 + tx, points[0].1 + ty),
                (points[1].0 + tx, points[1].1 + ty),
                (points[2].0 + tx, points[2].1 + ty),
                (points[3].0 + tx, points[3].1 + ty),
            ]
        };

        let uv_words = stack_word(seq_record(frame, plan.seq_uv), 0x04)
            .and_then(|word| word.snapshot.as_ref())
            .expect("uv snapshot");
        let uv_or_aux = words_as_pairs(&words_from_snapshot(uv_words));
        let aux_array = plan.seq_aux.map(|seq| {
            let aux_words = stack_word(seq_record(frame, seq), 0x04)
                .and_then(|word| word.snapshot.as_ref())
                .expect("aux snapshot");
            words_as_pairs(&words_from_snapshot(aux_words))
        });

        let screen_bounds = local_positions.iter().fold(
            (
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ),
            |acc, (x, y)| (acc.0.min(*x), acc.1.min(*y), acc.2.max(*x), acc.3.max(*y)),
        );

        let mut fb = vec![Rgba8::rgba(0, 0, 0, 0); 320 * 240];
        let coverage = rasterize_quad(
            &mut fb,
            320,
            240,
            &plan.texture,
            &local_positions,
            &[uv_or_aux[0], uv_or_aux[1], uv_or_aux[2], uv_or_aux[3]],
        );

        Self {
            ordinal159_handle,
            state_ptr,
            translation: (tx, ty),
            local_positions,
            uv_or_aux,
            aux_array,
            screen_bounds,
            proposed_texture: plan.proposed_texture,
            texture: plan.texture,
            coverage,
        }
    }

    fn rasterize(&self, fb: &mut [Rgba8]) -> u64 {
        let positions = self.local_positions;
        let uvs = [
            self.uv_or_aux[0],
            self.uv_or_aux[1],
            self.uv_or_aux[2],
            self.uv_or_aux[3],
        ];
        rasterize_quad(fb, 320, 240, &self.texture, &positions, &uvs)
    }
}

#[derive(Debug, Clone)]
struct ProposedTexture {
    label: &'static str,
    kind: &'static str,
    width: Option<usize>,
    height: Option<usize>,
    format: TextureFormat,
    confidence: f32,
    unresolved_note: Option<&'static str>,
}

impl ProposedTexture {
    fn resolved(
        label: &'static str,
        width: usize,
        height: usize,
        format: TextureFormat,
        confidence: f32,
    ) -> Self {
        Self {
            label,
            kind: "candidate",
            width: Some(width),
            height: Some(height),
            format,
            confidence,
            unresolved_note: None,
        }
    }

    fn unresolved(
        label: &'static str,
        note: &'static str,
        format: TextureFormat,
        confidence: f32,
    ) -> Self {
        Self {
            label,
            kind: "unresolved",
            width: None,
            height: None,
            format,
            confidence,
            unresolved_note: Some(note),
        }
    }
}

#[test]
fn fixed_16_16_decodes_signed_values() {
    assert_eq!(decode_fixed_16_16(0x0001_0000), 1.0);
    assert_eq!(decode_fixed_16_16(0x0000_8000), 0.5);
    assert_eq!(decode_fixed_16_16(0xffff_8000), -0.5);
    assert_eq!(decode_fixed_16_16(0x00ef_8000), 239.5);
}

#[test]
fn decodes_frame4_draw_stream_and_associations() {
    let fixture = load_fixture();
    let frame4 = first_frame(&fixture, 4).expect("steady-state frame");

    let draws = [
        (
            6u64,
            320u32,
            240u32,
            TextureFormat::Rgb565,
            0.93f32,
            "screenBG_565.pix",
            false,
        ),
        (
            20u64,
            250u32,
            162u32,
            TextureFormat::Rgba4444,
            0.84f32,
            "tetrisLogo_4444.pix",
            false,
        ),
        (
            31u64,
            50u32,
            50u32,
            TextureFormat::Rgba5551,
            0.87f32,
            "eaLogo_5551.pix",
            false,
        ),
        (
            41u64,
            1u32,
            1u32,
            TextureFormat::A8,
            0.28f32,
            "generated placeholder",
            true,
        ),
    ];

    let summaries = replay_frame4().1;
    assert_eq!(summaries.len(), 4);

    for (idx, (summary, (seq_159, width, height, format, confidence, label, unresolved))) in
        summaries.iter().zip(draws.iter()).enumerate()
    {
        assert!(summary.ordinal159_handle > 0);
        if idx == 0 {
            assert!(summary.aux_array.is_some());
        } else {
            assert!(summary.aux_array.is_none());
        }
        assert!(summary.state_ptr > 0);
        assert_eq!(
            summary.proposed_texture.kind,
            if *unresolved {
                "unresolved"
            } else {
                "candidate"
            }
        );
        assert_eq!(summary.proposed_texture.format, *format);
        assert!((summary.proposed_texture.confidence - confidence).abs() < 0.001);
        assert_eq!(summary.proposed_texture.label, *label);
        if *unresolved {
            assert_eq!(summary.proposed_texture.width, None);
            assert_eq!(summary.proposed_texture.height, None);
            assert_eq!(
                summary.proposed_texture.unresolved_note,
                Some("handle 3 / full-screen overlay")
            );
        } else {
            assert_eq!(summary.proposed_texture.width, Some(*width as usize));
            assert_eq!(summary.proposed_texture.height, Some(*height as usize));
            assert_eq!(summary.proposed_texture.unresolved_note, None);
        }
        assert!(summary.coverage > 0);
        assert!(summary.translation.0.is_finite());
        assert!(summary.translation.1.is_finite());
        assert!(summary.screen_bounds.0.is_finite());
        assert!(summary.screen_bounds.1.is_finite());
        assert!(summary.screen_bounds.2.is_finite());
        assert!(summary.screen_bounds.3.is_finite());
        let record = seq_record(frame4, *seq_159);
        assert_eq!(
            summary.ordinal159_handle,
            register(record, "r0").unwrap().value
        );
    }
}

#[test]
fn sample_nearest_matches_floor_and_clamp_capture_coordinates() {
    let tex = Texture::from_bytes(
        &[
            0x00, 0xf8, // top-left red
            0xe0, 0x07, // top-right green
            0x1f, 0x00, // bottom-left blue
            0xff, 0xff, // bottom-right white
        ],
        2,
        2,
        TextureFormat::Rgb565,
        Rgba8::rgba(255, 255, 255, 255),
    );
    assert_eq!(sample_nearest(&tex, 0.5, 0.5), Rgba8::rgba(255, 0, 0, 255));
    assert_eq!(sample_nearest(&tex, 1.5, 0.5), Rgba8::rgba(0, 255, 0, 255));
    assert_eq!(sample_nearest(&tex, 0.5, 1.5), Rgba8::rgba(0, 0, 255, 255));
    assert_eq!(
        sample_nearest(&tex, 1.5, 1.5),
        Rgba8::rgba(255, 255, 255, 255)
    );
    assert_eq!(
        sample_nearest(&tex, -0.5, -0.5),
        Rgba8::rgba(255, 0, 0, 255)
    );
}

#[test]
fn rasterizer_supports_formats_alpha_clipping_and_winding() {
    let transparent = Texture::from_bytes(
        &[0x00],
        1,
        1,
        TextureFormat::A8,
        Rgba8::rgba(255, 0, 0, 255),
    );
    let opaque_red = Texture::from_bytes(
        &[0xff],
        1,
        1,
        TextureFormat::A8,
        Rgba8::rgba(255, 0, 0, 255),
    );
    let rgba4444 = Texture::from_bytes(
        &[0x08, 0xf0],
        1,
        1,
        TextureFormat::Rgba4444,
        Rgba8::rgba(255, 255, 255, 255),
    );
    let rgba5551 = Texture::from_bytes(
        &[0x01, 0xf8],
        1,
        1,
        TextureFormat::Rgba5551,
        Rgba8::rgba(255, 255, 255, 255),
    );
    let rgb565 = Texture::from_bytes(
        &[0x1f, 0x00],
        1,
        1,
        TextureFormat::Rgb565,
        Rgba8::rgba(255, 255, 255, 255),
    );

    assert_eq!(
        blend_src_over(Rgba8::rgba(10, 20, 30, 255), Rgba8::rgba(0, 0, 0, 0)),
        Rgba8::rgba(10, 20, 30, 255)
    );
    assert_eq!(
        blend_src_over(Rgba8::rgba(10, 20, 30, 255), Rgba8::rgba(200, 10, 50, 255)),
        Rgba8::rgba(200, 10, 50, 255)
    );

    let mut fb = vec![Rgba8::rgba(0, 0, 0, 0); 4 * 4];
    let quad = [(0.0, 0.0), (0.0, 2.0), (2.0, 2.0), (2.0, 0.0)];
    let uvs = [(0.5, 0.5), (0.5, 0.5), (0.5, 0.5), (0.5, 0.5)];
    let cov = rasterize_quad(&mut fb, 4, 4, &opaque_red, &quad, &uvs);
    assert_eq!(cov, 4);
    assert_eq!(fb[0], Rgba8::rgba(255, 0, 0, 255));

    let mut fb = vec![Rgba8::rgba(0, 0, 255, 255); 2 * 2];
    let quad = [(-1.0, -1.0), (-1.0, 1.0), (1.0, 1.0), (1.0, -1.0)];
    let uvs = [(0.5, 0.5), (0.5, 0.5), (0.5, 0.5), (0.5, 0.5)];
    let cov = rasterize_quad(&mut fb, 2, 2, &transparent, &quad, &uvs);
    assert_eq!(cov, 1);
    assert_eq!(fb[0], Rgba8::rgba(0, 0, 255, 255));

    let mut fb = vec![Rgba8::rgba(0, 0, 0, 0); 2 * 2];
    let quad = [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)];
    let uvs = [(0.5, 0.5), (0.5, 0.5), (0.5, 0.5), (0.5, 0.5)];
    let cov_rgba4444 = rasterize_quad(&mut fb, 2, 2, &rgba4444, &quad, &uvs);
    assert_eq!(cov_rgba4444, 1);
    assert_eq!(rgba4444.pixels[0].a, 136);
    assert_eq!(fb[0], rgba4444.pixels[0]);

    let mut fb_blend = vec![Rgba8::rgba(0, 0, 255, 255); 2 * 2];
    let cov_rgba4444_blend = rasterize_quad(&mut fb_blend, 2, 2, &rgba4444, &quad, &uvs);
    assert_eq!(cov_rgba4444_blend, 1);
    assert_eq!(
        fb_blend[0],
        blend_src_over(Rgba8::rgba(0, 0, 255, 255), rgba4444.pixels[0])
    );

    let mut fb = vec![Rgba8::rgba(0, 0, 0, 0); 2 * 2];
    let cov_rgba5551 = rasterize_quad(&mut fb, 2, 2, &rgba5551, &quad, &uvs);
    assert_eq!(cov_rgba5551, 1);
    assert_eq!(fb[0].a, 255);

    let mut fb = vec![Rgba8::rgba(0, 0, 0, 0); 2 * 2];
    let cov_rgb565 = rasterize_quad(&mut fb, 2, 2, &rgb565, &quad, &uvs);
    assert_eq!(cov_rgb565, 1);
    assert_eq!(fb[0].a, 255);

    let a8_mask = Texture::from_bytes(
        &[128],
        1,
        1,
        TextureFormat::A8,
        Rgba8::rgba(40, 80, 160, 255),
    );
    let mut fb = vec![Rgba8::rgba(0, 0, 0, 0); 1];
    let cov_a8 = rasterize_quad(
        &mut fb,
        1,
        1,
        &a8_mask,
        &[(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)],
        &uvs,
    );
    assert_eq!(cov_a8, 1);
    assert_eq!(fb[0], Rgba8::rgba(40, 80, 160, 128));

    let mut fb_a = vec![Rgba8::rgba(0, 0, 0, 0); 4 * 4];
    let mut fb_b = vec![Rgba8::rgba(0, 0, 0, 0); 4 * 4];
    let quad = [(0.0, 0.0), (0.0, 2.0), (2.0, 2.0), (2.0, 0.0)];
    let uvs = [(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)];
    let tex = Texture::from_bytes(
        &[
            0x00, 0xf8, // red
            0xe0, 0x07, // green
            0x1f, 0x00, // blue
            0xff, 0xff, // white
        ],
        2,
        2,
        TextureFormat::Rgb565,
        Rgba8::rgba(255, 255, 255, 255),
    );
    let cov_a = rasterize_quad(&mut fb_a, 4, 4, &tex, &quad, &uvs);
    let cov_b = rasterize_quad(
        &mut fb_b,
        4,
        4,
        &tex,
        &[(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)],
        &[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
    );
    assert_eq!(cov_a, cov_b);
    assert_eq!(fb_a, fb_b);
}

#[test]
fn replay_frame4_produces_complete_artifact_and_hash() {
    let (fb, draws) = replay_frame4();
    assert_eq!(draws.len(), 4);

    let mut bytes = Vec::with_capacity(fb.len() * 4);
    for px in &fb {
        bytes.extend_from_slice(&[px.r, px.g, px.b, px.a]);
    }
    let hash = framebuffer_hash(&fb);
    if std::env::var_os("CLICKY_WRITE_TETRIS_FRAME4_PPM").is_some() {
        framebuffer_to_ppm(
            std::path::Path::new("/tmp/tetris_frame4_replay.ppm"),
            &fb,
            320,
            240,
        );
    }
    if std::env::var_os("CLICKY_PRINT_FRAME4_SUMMARY").is_some() {
        for (idx, draw) in draws.iter().enumerate() {
            println!(
                "draw{} handle={} cov={} bounds=({:.1},{:.1})-({:.1},{:.1}) tex={} kind={} format={:?}",
                idx + 1,
                draw.ordinal159_handle,
                draw.coverage,
                draw.screen_bounds.0,
                draw.screen_bounds.1,
                draw.screen_bounds.2,
                draw.screen_bounds.3,
                draw.proposed_texture.label,
                draw.proposed_texture.kind,
                draw.proposed_texture.format,
            );
        }
        println!("frame4_hash={:016x}", hash);
    }

    if std::env::var_os("CLICKY_WRITE_TETRIS_FRAME4_PPM").is_some() {
        let (_, base_draws) = replay_frame4();
        let draws_1_to_3_fb = render_draws(&base_draws, false);
        let all_draws_fb = render_draws(&base_draws, true);
        let draw4_alpha = replay_frame4_with_probe(Draw4ProbeMode::AlphaOnly);
        let draw4_alpha_fb = render_draws(&draw4_alpha[3..4], true);
        let draw4_opaque = replay_frame4_with_probe(Draw4ProbeMode::Opaque);
        let draw4_opaque_fb = render_draws(&draw4_opaque[3..4], true);

        write_frame4_ppm_if_requested("/tmp/tetris_frame4_draws_1_3.ppm", &draws_1_to_3_fb);
        write_frame4_ppm_if_requested("/tmp/tetris_frame4_all_draws.ppm", &all_draws_fb);
        write_frame4_ppm_if_requested("/tmp/tetris_frame4_draw4_alpha.ppm", &draw4_alpha_fb);
        write_frame4_ppm_if_requested("/tmp/tetris_frame4_draw4_opaque.ppm", &draw4_opaque_fb);
    }

    assert_eq!(hash, 0x3514_598d_ae7f_1fe2);
    assert_eq!(bytes.len(), 320 * 240 * 4);
}

#[derive(Debug, Copy, Clone)]
enum Draw4ProbeMode {
    AlphaOnly,
    Opaque,
}

fn draw4_probe_texture(mode: Draw4ProbeMode) -> (Texture, ProposedTexture) {
    match mode {
        Draw4ProbeMode::AlphaOnly => (
            make_texture(TextureFormat::A8, 1, 1, make_raw_a8(1, 1)),
            ProposedTexture::unresolved(
                "handle 3 / full-screen overlay",
                "identity/overlay probe; not a final asset mapping",
                TextureFormat::A8,
                0.28,
            ),
        ),
        Draw4ProbeMode::Opaque => (
            make_texture(TextureFormat::Rgba5551, 1, 1, vec![0xff, 0xff]),
            ProposedTexture::unresolved(
                "handle 3 / full-screen overlay",
                "identity/overlay probe; not a final asset mapping",
                TextureFormat::Rgba5551,
                0.28,
            ),
        ),
    }
}

fn replay_frame4_with_probe(mode: Draw4ProbeMode) -> Vec<DrawReplay> {
    let (_, mut draws) = replay_frame4();
    let (texture, proposed_texture) = draw4_probe_texture(mode);
    draws[3].texture = texture;
    draws[3].proposed_texture = proposed_texture;
    draws
}

fn render_draws(draws: &[DrawReplay], include_draw4: bool) -> Vec<Rgba8> {
    let mut fb = vec![Rgba8::rgba(0, 0, 0, 0); 320 * 240];
    for (idx, draw) in draws.iter().enumerate() {
        if !include_draw4 && idx == 3 {
            continue;
        }
        draw.rasterize(&mut fb);
    }
    fb
}

fn framebuffer_stats(
    fb: &[Rgba8],
    width: usize,
    height: usize,
) -> (usize, Option<(usize, usize, usize, usize)>, (u8, u8)) {
    let mut nonzero = 0usize;
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    let mut alpha_min = u8::MAX;
    let mut alpha_max = 0u8;

    for y in 0..height {
        for x in 0..width {
            let px = fb[y * width + x];
            if px.r != 0 || px.g != 0 || px.b != 0 || px.a != 0 {
                nonzero += 1;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
            alpha_min = alpha_min.min(px.a);
            alpha_max = alpha_max.max(px.a);
        }
    }

    (
        nonzero,
        if nonzero == 0 {
            None
        } else {
            Some((min_x, min_y, max_x, max_y))
        },
        (alpha_min, alpha_max),
    )
}

fn diff_pixels(a: &[Rgba8], b: &[Rgba8]) -> usize {
    a.iter()
        .zip(b.iter())
        .filter(|(lhs, rhs)| lhs != rhs)
        .count()
}

fn write_frame4_ppm_if_requested(path: &str, fb: &[Rgba8]) {
    if std::env::var_os("CLICKY_WRITE_TETRIS_FRAME4_PPM").is_some() {
        framebuffer_to_ppm(std::path::Path::new(path), fb, 320, 240);
        println!("ppm_path={path} hash={:016x}", framebuffer_hash(fb));
    }
}

fn print_draw_summary(
    name: &str,
    fb: &[Rgba8],
    draws: &[DrawReplay],
    include_draw4: bool,
    draw_index_offset: usize,
) {
    let (nonzero, bbox, alpha_range) = framebuffer_stats(fb, 320, 240);
    println!("artifact={name}");
    println!("  hash={:016x}", framebuffer_hash(fb));
    println!("  nonzero_pixels={nonzero}");
    match bbox {
        Some((min_x, min_y, max_x, max_y)) => {
            println!("  bbox=({}, {})-({}, {})", min_x, min_y, max_x, max_y)
        }
        None => println!("  bbox=none"),
    }
    println!("  alpha_range=({}, {})", alpha_range.0, alpha_range.1);
    for (idx, draw) in draws.iter().enumerate() {
        if !include_draw4 && idx == 3 {
            continue;
        }
        println!(
            "  draw{} handle={} coverage={} bounds=({:.1},{:.1})-({:.1},{:.1}) tex={} kind={} format={:?}",
            idx + 1 + draw_index_offset,
            draw.ordinal159_handle,
            draw.coverage,
            draw.screen_bounds.0,
            draw.screen_bounds.1,
            draw.screen_bounds.2,
            draw.screen_bounds.3,
            draw.proposed_texture.label,
            draw.proposed_texture.kind,
            draw.proposed_texture.format,
        );
    }
}

#[test]
fn frame4_artifact_comparison_and_handle_mapping() {
    let fixture = load_fixture();
    let uploads = texture_upload_candidates(&fixture);
    let (_, base_draws) = replay_frame4();
    let draws_1_to_3 = render_draws(&base_draws, false);
    let all_draws = render_draws(&base_draws, true);

    let draw4_alpha = replay_frame4_with_probe(Draw4ProbeMode::AlphaOnly);
    let draw4_alpha_fb = render_draws(&draw4_alpha[3..4], true);
    let draw4_opaque = replay_frame4_with_probe(Draw4ProbeMode::Opaque);
    let draw4_opaque_fb = render_draws(&draw4_opaque[3..4], true);
    let draw4_only = render_draws(&base_draws[3..4], true);

    if std::env::var_os("CLICKY_PRINT_FRAME4_ARTIFACTS").is_some() {
        print_draw_summary("draws_1_to_3_only", &draws_1_to_3, &base_draws, false, 0);
        print_draw_summary(
            "all_draws_draw4_disabled",
            &draws_1_to_3,
            &base_draws,
            false,
            0,
        );
        print_draw_summary("all_draws_placeholder", &all_draws, &base_draws, true, 0);
        println!(
            "  overwrite_vs_draws_1_to_3={} diff_pixels={}",
            if diff_pixels(&draws_1_to_3, &all_draws) > 0 {
                "yes"
            } else {
                "no"
            },
            diff_pixels(&draws_1_to_3, &all_draws)
        );

        print_draw_summary(
            "draw4_only_placeholder",
            &draw4_only,
            &base_draws[3..4],
            true,
            3,
        );
        print_draw_summary(
            "draw4_only_alpha",
            &draw4_alpha_fb,
            &draw4_alpha[3..4],
            true,
            3,
        );
        print_draw_summary(
            "draw4_only_opaque",
            &draw4_opaque_fb,
            &draw4_opaque[3..4],
            true,
            3,
        );
    }

    write_frame4_ppm_if_requested("/tmp/tetris_frame4_draws_1_3.ppm", &draws_1_to_3);
    write_frame4_ppm_if_requested("/tmp/tetris_frame4_all_draws.ppm", &all_draws);
    write_frame4_ppm_if_requested("/tmp/tetris_frame4_draw4_alpha.ppm", &draw4_alpha_fb);
    write_frame4_ppm_if_requested("/tmp/tetris_frame4_draw4_opaque.ppm", &draw4_opaque_fb);

    // Conservative handle mapping from upload candidates to frame-4 ord159 draws.
    let mapping_rows = [
        (
            "screenBG_565.pix",
            19u32,
            "frame4 draw1",
            Some(0.93f32),
            "exact table write not captured; matched by size + fullscreen state blob",
        ),
        (
            "tetrisLogoT_4444.pix",
            14u32,
            "frame4 draw2",
            Some(0.84f32),
            "exact table write not captured; matched by size + state blob",
        ),
        (
            "eaLogo_5551.pix",
            27u32,
            "frame4 draw3",
            Some(0.87f32),
            "exact table write not captured; matched by size + state blob",
        ),
        (
            "no upload candidate",
            3u32,
            "frame4 draw4",
            Some(0.28f32),
            "no matching upload triplet; appears to be a generated fullscreen overlay/material blob",
        ),
    ];

    if std::env::var_os("CLICKY_PRINT_FRAME4_MAPPING").is_some() {
        println!("mapping_table");
        for row in &mapping_rows {
            println!(
                "  source_file={} handle={} draw={} confidence={:.2} missing={}",
                row.0,
                row.1,
                row.2,
                row.3.unwrap_or(0.0),
                row.4,
            );
        }
        println!("  uploads={}", uploads.len());
        for upload in uploads.iter().take(4) {
            println!(
                "  upload seqs={}→{}→{} file={} desc={:#x} object_tag={} target={:#x} fmt={:#x} type={:#x} src={:#x}",
                upload.ordinal45_seq,
                upload.ordinal4_seq,
                upload.ordinal99_seq,
                upload
                    .source_file
                    .as_ref()
                    .map(|f| f.path.as_str())
                    .unwrap_or("<unknown>"),
                upload.descriptor_ptr,
                upload.object_tag,
                upload.target,
                upload.internal_format,
                upload.pixel_type,
                upload.source_ptr,
            );
        }
    }

    assert!(diff_pixels(&draws_1_to_3, &all_draws) > 0);
    assert_eq!(draw4_only.len(), 320 * 240);
    assert_eq!(draw4_alpha_fb.len(), 320 * 240);
    assert_eq!(draw4_opaque_fb.len(), 320 * 240);
}
