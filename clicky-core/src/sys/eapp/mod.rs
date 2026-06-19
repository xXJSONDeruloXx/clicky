use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use armv4t_emu::{reg, Cpu, Mode as ArmMode};
use thiserror::Error;

mod gl_decode;
mod gl_trace;
mod live_gl;
mod rasterizer;
pub use gl_decode::{
    bytes_from_snapshot, decode_fixed_16_16, first_frame, fixed_words_from_snapshot,
    float_words_from_snapshot, format_from_gl, pix_payload_size, register, stack_word,
    texture_upload_candidates, words_from_snapshot, TextureUploadCandidate,
};
use gl_trace::hex_bytes;
pub use gl_trace::{
    GlFileBacking, GlFrameRecord, GlImportRecord, GlMemoryRegion, GlMemorySnapshot,
    GlRegisterSnapshot, GlStackWordSnapshot, GlTraceFixture, GlTraceRecorder, GlValueClass,
};
use live_gl::LiveGlState;
pub use rasterizer::{
    blend_src_over, decode_texture_pixels, framebuffer_hash, framebuffer_to_ppm, rasterize_quad,
    rasterize_triangle, sample_nearest, Rgba8, Texture, TextureFormat,
};

use crate::devices::generic::Ram;
use crate::devices::{Device, Probe};
use crate::error::*;
use crate::gui::{ButtonCallback, RenderCallback, ScrollCallback, TakeControls};
use crate::memory::{armv4t_adaptor::MemoryAdapter, Memory};

const FILE_VMA_BASE: u32 = 0x1800_0000;
const RECENT_PC_LIMIT: usize = 64;
const BOOTSTRAP_RETURN_PC: u32 = 0x1eff_fffc;
const GUEST_CALLBACK_RETURN_PC: u32 = 0x1eff_fff8;
const WORK_RAM_BASE: u32 = 0x1000_0000;
const WORK_RAM_SIZE: usize = 8 * 1024 * 1024;
const STACK_TOP: u32 = WORK_RAM_BASE + WORK_RAM_SIZE as u32 - 0x1000;
const TRAMPOLINE_BASE: u32 = 0x1f00_0000;
const TRAMPOLINE_STRIDE: u32 = 0x20;
const SCREEN_WIDTH: usize = 320;
const SCREEN_HEIGHT: usize = 240;
const SCREEN_PIXELS: usize = SCREEN_WIDTH * SCREEN_HEIGHT;
const IMAGE_RAM_SLACK: usize = 2 * 1024 * 1024;
const EAPP_HEADER_SIZE: usize = 0x28;
const IMPORT_NAME_LEN: usize = 0x20;
const IMPORT_COUNT_OFFSET: usize = 0x30;
const IMPORT_NEXT_OFFSET: usize = 0x34;
const IMPORT_STUBS_OFFSET: usize = 0x38;
const IMPORT_SENTINEL_NAME: &str = "$$$$ a^n + b^n = c^n | n>2 $$$$";
const DEFAULT_FRAMEBUFFER: u32 = 0xff101820;
const HLE_INFO_FRAMEBUFFER: u32 = 0xff203040;
const HLE_WARN_FRAMEBUFFER: u32 = 0xff604020;
const HLE_OPENGL_FRAMEBUFFER: u32 = 0xff205020;

#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub enum EappKey {
    Up,
    Down,
    Left,
    Right,
    Action,
    Menu,
}

#[derive(Default)]
pub struct EappBinds {
    pub keys: HashMap<EappKey, ButtonCallback>,
    pub wheel: Option<ScrollCallback>,
}

#[derive(Debug, Default, Clone)]
pub struct EappInputState {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub action: bool,
    pub menu: bool,
    pub wheel_delta: f32,
}

#[derive(Debug, Clone)]
pub struct EappMetadata {
    pub title: String,
    pub bundle_dir: PathBuf,
    pub executable_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct EappHeader {
    pub load_addr_guess: u32,
    pub format_version: u32,
    pub header_size: u32,
    pub imports_addr: u32,
    pub entry_addr: u32,
    pub init_addr: u32,
    pub aux_addr: u32,
}

#[derive(Debug, Clone)]
pub struct EappImportModule {
    pub name_addr: u32,
    pub name: String,
    pub count: u32,
    pub next_addr: u32,
    pub stubs_addr: u32,
    pub literals_addr: u32,
}

#[derive(Debug, Clone)]
pub struct EappImage {
    pub metadata: EappMetadata,
    pub header: EappHeader,
    pub imports: Vec<EappImportModule>,
    pub image: Vec<u8>,
}

#[derive(Debug, Clone)]
struct BoundImport {
    module: String,
    ordinal: u32,
}

#[derive(Debug, Clone)]
struct StartupProgressTrace {
    enabled: bool,
    max_logs: usize,
    interval: u64,
    logged: usize,
    last_framebuffer_hash: Option<u64>,
    first_hash_change_frame: Option<u64>,
}

impl StartupProgressTrace {
    fn from_env() -> StartupProgressTrace {
        let enabled = std::env::var_os("CLICKY_STARTUP_PROGRESS_TRACE")
            .map(|v| v.to_string_lossy() == "1")
            .unwrap_or(false);
        let max_logs = std::env::var_os("CLICKY_STARTUP_PROGRESS_FRAMES")
            .and_then(|v| v.to_string_lossy().parse::<usize>().ok())
            .unwrap_or(180);
        let interval = std::env::var_os("CLICKY_STARTUP_PROGRESS_INTERVAL")
            .and_then(|v| v.to_string_lossy().parse::<u64>().ok())
            .unwrap_or(60);
        StartupProgressTrace {
            enabled,
            max_logs,
            interval: interval.max(1),
            logged: 0,
            last_framebuffer_hash: None,
            first_hash_change_frame: None,
        }
    }
}

#[derive(Debug, Clone)]
struct StartupArtifactCapture {
    enabled: bool,
    dir: PathBuf,
    manifest_path: PathBuf,
    periodic_interval: u64,
    max_frames: u64,
    max_dumps: u64,
    manifest_rows: u64,
    dump_count: u64,
    last_hash: Option<u64>,
}

impl StartupArtifactCapture {
    fn from_env() -> StartupArtifactCapture {
        let enabled = std::env::var_os("CLICKY_STARTUP_CAPTURE_DIR").is_some();
        let dir = std::env::var_os("CLICKY_STARTUP_CAPTURE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp/clicky_tetris_startup_capture"));
        let manifest_path = dir.join("manifest.tsv");
        let periodic_interval = std::env::var_os("CLICKY_STARTUP_CAPTURE_PERIOD")
            .and_then(|v| v.to_string_lossy().parse::<u64>().ok())
            .unwrap_or(30)
            .max(1);
        let max_frames = std::env::var_os("CLICKY_STARTUP_CAPTURE_MAX_FRAMES")
            .and_then(|v| v.to_string_lossy().parse::<u64>().ok())
            .unwrap_or(1200);
        let max_dumps = std::env::var_os("CLICKY_STARTUP_CAPTURE_MAX_DUMPS")
            .and_then(|v| v.to_string_lossy().parse::<u64>().ok())
            .unwrap_or(400);
        if enabled {
            let _ = fs::create_dir_all(&dir);
            let _ = fs::write(
                &manifest_path,
                "guest_frame\thost_us\tguest_time_current\tguest_time_delta\tdraw_count\thandles\tinternal_hash\tpresented_hash\tdump_reason\tpath\n",
            );
        }
        StartupArtifactCapture {
            enabled,
            dir,
            manifest_path,
            periodic_interval,
            max_frames,
            max_dumps,
            manifest_rows: 0,
            dump_count: 0,
            last_hash: None,
        }
    }
}

pub struct Eapp {
    cpu: Cpu,
    bus: EappBus,
    metadata: EappMetadata,
    header: EappHeader,
    imports: Vec<BoundImport>,
    trampoline_to_import: HashMap<u32, usize>,
    logged_imports: HashSet<(String, u32)>,
    recent_pcs: VecDeque<u32>,
    input_state: Arc<Mutex<EappInputState>>,
    render_state: Arc<Mutex<Vec<u32>>>,
    controls: Option<EappBinds>,
    next_alloc: u32,
    bootstrap_phase: BootstrapPhase,
    app_object: u32,
    frame_context: u32,
    frame_counter: u64,
    pending_guest_calls: VecDeque<PendingGuestCall>,
    /// Host file contents staged for delivery to the guest, keyed by the guest
    /// request-object address that asked for them.
    staged_files: HashMap<u32, StagedFile>,
    /// Request objects we've already dumped once, to keep logs tractable.
    dumped_requests: HashSet<u32>,
    /// Per-(module, ordinal) call counters, to find render-critical imports.
    import_call_counts: HashMap<(String, u32), u64>,
    /// Per-frame import counters used by the optional startup-progress trace.
    frame_import_counts: HashMap<(String, u32), u64>,
    startup_progress: StartupProgressTrace,
    startup_capture: StartupArtifactCapture,
    startup_signature_reports: HashSet<String>,
    host_start: Instant,
    misc9_time_diag_count: u64,
    misc9_last_pointed_value: Option<u32>,
    async_request_count: u64,
    async_callback_queued_count: u64,
    guest_callback_invocation_count: u64,
    async_pending_requests: HashSet<u32>,
    /// Optional inclusive frame window in which to log every OpenGLES call
    /// with full args + return address, for reverse-engineering the GL stream.
    gl_trace_frames: Option<(u64, u64)>,
    /// Optional bounded OpenGLES capture recorder for machine-readable traces.
    gl_capture: Option<GlTraceRecorder>,
    staged_file_generation: u64,
    halted: bool,
    /// Optional live OpenGLES HLE state. Present only when
    /// `CLICKY_EXPERIMENTAL_GL_HLE=1`; when `None` the legacy fill-color
    /// GL path is used unchanged.
    live_gl: Option<LiveGlState>,
}

#[derive(Debug, Clone)]
struct StagedFile {
    /// Monotonic host-side generation so overlapping reused buffers can be
    /// attributed to the most recent AsyncFileIO delivery.
    generation: u64,
    /// Guest address where the file payload bytes have been copied.
    payload_addr: u32,
    /// Length in bytes.
    len: u32,
    /// Host path the bytes came from.
    host_path: PathBuf,
}

#[derive(Debug, Copy, Clone)]
struct PendingGuestCall {
    pc: u32,
    arg0: u32,
    arg1: u32,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum BootstrapPhase {
    Entry,
    Running,
    Done,
}

#[derive(Debug)]
struct EappBus {
    image: Ram,
    image_len: u32,
    work_ram: Ram,
}

#[derive(Error, Debug)]
pub enum EappBuildError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not find an executable under {0}")]
    MissingExecutable(String),
    #[error("invalid eapp image: {0}")]
    InvalidImage(String),
}

impl Eapp {
    pub fn from_bundle_dir(bundle_dir: impl AsRef<Path>) -> Result<Eapp, EappBuildError> {
        let bundle_dir = bundle_dir.as_ref().to_path_buf();
        let executable_path = find_game_executable(&bundle_dir)?;
        let title = bundle_dir
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| executable_path.display().to_string());
        let metadata = EappMetadata {
            title,
            bundle_dir,
            executable_path,
        };
        let image = EappImage::load(metadata)?;
        Eapp::from_image(image)
    }

    pub fn from_image(image: EappImage) -> Result<Eapp, EappBuildError> {
        let render_state = Arc::new(Mutex::new(vec![DEFAULT_FRAMEBUFFER; SCREEN_PIXELS]));
        let input_state = Arc::new(Mutex::new(EappInputState::default()));
        let controls = make_controls(Arc::clone(&input_state));

        let mut cpu = Cpu::new();
        cpu.reg_set(ArmMode::User, reg::PC, image.header.entry_addr);
        cpu.reg_set(ArmMode::User, reg::CPSR, 0xd3);
        cpu.reg_set(ArmMode::Supervisor, reg::SP, STACK_TOP);
        cpu.reg_set(ArmMode::User, reg::LR, BOOTSTRAP_RETURN_PC);

        let mut patched_image = image.image.clone();
        let mut imports = Vec::new();
        let mut trampoline_to_import = HashMap::new();
        let mut trampoline_addr = TRAMPOLINE_BASE;

        for module in &image.imports {
            for ordinal in 0..module.count {
                let import_idx = imports.len();
                let literal_addr = module.literals_addr + ordinal * 4;
                let literal_offset = vma_to_offset(literal_addr)? as usize;
                patched_image[literal_offset..literal_offset + 4]
                    .copy_from_slice(&trampoline_addr.to_le_bytes());

                imports.push(BoundImport {
                    module: module.name.clone(),
                    ordinal,
                });
                trampoline_to_import.insert(trampoline_addr, import_idx);
                trampoline_addr = trampoline_addr.wrapping_add(TRAMPOLINE_STRIDE);
            }
        }

        let mapped_image_len = patched_image.len() + IMAGE_RAM_SLACK;
        let mut image_ram = Ram::new(mapped_image_len);
        let image_zeroes = vec![0u8; mapped_image_len];
        image_ram.bulk_write(0, &image_zeroes);
        image_ram.bulk_write(0, &patched_image);

        let mut work_ram = Ram::new(WORK_RAM_SIZE);
        let zeroes = vec![0u8; WORK_RAM_SIZE];
        work_ram.bulk_write(0, &zeroes);

        Ok(Eapp {
            cpu,
            bus: EappBus {
                image: image_ram,
                image_len: mapped_image_len as u32,
                work_ram,
            },
            metadata: image.metadata,
            header: image.header,
            imports,
            trampoline_to_import,
            logged_imports: HashSet::new(),
            recent_pcs: VecDeque::with_capacity(RECENT_PC_LIMIT),
            input_state,
            render_state,
            controls: Some(controls),
            next_alloc: WORK_RAM_BASE + 0x1000,
            bootstrap_phase: BootstrapPhase::Entry,
            app_object: 0,
            frame_context: 0,
            frame_counter: 0,
            pending_guest_calls: VecDeque::new(),
            staged_files: HashMap::new(),
            dumped_requests: HashSet::new(),
            import_call_counts: HashMap::new(),
            frame_import_counts: HashMap::new(),
            startup_progress: StartupProgressTrace::from_env(),
            startup_capture: StartupArtifactCapture::from_env(),
            startup_signature_reports: HashSet::new(),
            host_start: Instant::now(),
            misc9_time_diag_count: 0,
            misc9_last_pointed_value: None,
            async_request_count: 0,
            async_callback_queued_count: 0,
            guest_callback_invocation_count: 0,
            async_pending_requests: HashSet::new(),
            gl_trace_frames: None,
            gl_capture: None,
            staged_file_generation: 0,
            halted: false,
            live_gl: Self::maybe_init_live_gl(),
        })
    }

    pub fn title(&self) -> &str {
        &self.metadata.title
    }

    pub fn metadata(&self) -> &EappMetadata {
        &self.metadata
    }

    pub fn render_callback(&self) -> RenderCallback {
        let render_state = Arc::clone(&self.render_state);
        Box::new(move |buf: &mut Vec<u32>| -> (usize, usize) {
            let frame = render_state.lock().unwrap();
            buf.splice(.., frame.iter().copied());
            (SCREEN_WIDTH, SCREEN_HEIGHT)
        })
    }

    pub fn run(&mut self) -> FatalMemResult<()> {
        while !self.halted {
            self.step()?;
        }
        Ok(())
    }

    pub fn run_cycles(&mut self, cycles: usize) -> FatalMemResult<()> {
        for _ in 0..cycles {
            if self.halted {
                break;
            }
            self.step()?;
        }
        Ok(())
    }

    /// Log the most-frequent import calls seen so far. Useful for finding
    /// render-critical ordinals inside the per-frame loop.
    pub fn log_top_imports(&self, limit: usize) {
        let mut counts: Vec<(&(String, u32), &u64)> = self.import_call_counts.iter().collect();
        counts.sort_by(|a, b| b.1.cmp(a.1));
        let mut rendered = String::new();
        for ((module, ordinal), count) in counts.into_iter().take(limit) {
            rendered.push_str(&format!("\n    {}:{} = {}", module, ordinal, count));
        }
        info!(target: "EAPP", "top {} imports by call count:{}", limit, rendered);
    }

    /// Set an inclusive frame window in which to log every OpenGLES call with
    /// full args + return address. Used for Option A diagnostics.
    pub fn set_gl_trace_window(&mut self, start: u64, end: u64) {
        self.gl_trace_frames = Some((start, end));
    }

    /// Enable bounded JSON-friendly OpenGLES trace capture.
    pub fn enable_gl_capture(
        &mut self,
        start_frame: u64,
        end_frame: u64,
        stack_snapshot_len: usize,
        pointer_snapshot_len: usize,
    ) {
        self.gl_capture = Some(GlTraceRecorder::new(
            start_frame,
            end_frame,
            stack_snapshot_len,
            pointer_snapshot_len,
        ));
    }

    /// Drain the current GL capture into a fixture with metadata filled in.
    pub fn take_gl_trace_fixture(&mut self) -> Option<GlTraceFixture> {
        let recorder = self.gl_capture.take()?;
        let mut fixture = recorder.finalize();
        fixture.title = self.metadata.title.clone();
        fixture.bundle_dir = self.metadata.bundle_dir.display().to_string();
        fixture.executable_path = self.metadata.executable_path.display().to_string();
        fixture.file_vma_base = FILE_VMA_BASE;
        fixture.work_ram_base = WORK_RAM_BASE;
        fixture.work_ram_size = WORK_RAM_SIZE;
        Some(fixture)
    }

    /// Serialize the active GL capture as JSON.
    pub fn write_gl_trace_fixture(&mut self, path: impl AsRef<Path>) -> Result<(), std::io::Error> {
        let fixture = match self.take_gl_trace_fixture() {
            Some(fixture) => fixture,
            None => return Ok(()),
        };
        let json = serde_json::to_vec_pretty(&fixture).map_err(|err| {
            std::io::Error::new(std::io::ErrorKind::Other, format!("serde_json: {}", err))
        })?;
        fs::write(path, json)
    }

    fn capture_open_gl_import(&mut self, ordinal: u32, pc: u32, lr: u32, args: [u32; 4], ret: u32) {
        let Some((start, end)) = self.gl_capture.as_ref().map(|r| r.capture_range()) else {
            return;
        };
        if self.frame_counter < start || self.frame_counter > end {
            return;
        }

        let stack_len = self
            .gl_capture
            .as_ref()
            .map(|r| r.stack_snapshot_len())
            .unwrap_or(0x80);
        let pointer_len = self
            .gl_capture
            .as_ref()
            .map(|r| r.pointer_snapshot_len())
            .unwrap_or(0x80);
        let sp = self.cpu.reg_get(self.cpu.mode(), reg::SP);
        let registers = self.capture_registers(pc, lr, sp, args, pointer_len);
        let (stack, stack_bytes) = self.snapshot_memory_with_bytes(sp, stack_len);
        let stack_words = self.capture_stack_words(&stack_bytes, pointer_len);
        let record = GlImportRecord {
            seq: 0,
            seq_in_frame: 0,
            frame: self.frame_counter,
            ordinal,
            pc,
            lr,
            sp,
            return_value: ret,
            stack,
            stack_words,
            registers,
        };

        if let Some(recorder) = self.gl_capture.as_mut() {
            recorder.capture_record(self.frame_counter, record);
        }
    }

    fn capture_registers(
        &mut self,
        pc: u32,
        lr: u32,
        sp: u32,
        args: [u32; 4],
        pointer_len: usize,
    ) -> Vec<GlRegisterSnapshot> {
        let mut registers = Vec::with_capacity(16);
        for idx in 0..13u32 {
            let value = if idx < 4 {
                args[idx as usize]
            } else {
                self.cpu.reg_get(self.cpu.mode(), idx as u8)
            };
            registers.push(self.capture_register(format!("r{}", idx), value, pointer_len, idx < 4));
        }
        registers.push(self.capture_register("sp", sp, pointer_len, true));
        registers.push(self.capture_register("lr", lr, pointer_len, false));
        registers.push(self.capture_register("pc", pc, pointer_len, false));
        registers
    }

    fn capture_register(
        &mut self,
        name: impl Into<String>,
        value: u32,
        pointer_len: usize,
        allow_snapshot: bool,
    ) -> GlRegisterSnapshot {
        let name = name.into();
        let class = self.classify_trace_value(value);
        let float_value = matches!(class, GlValueClass::Float).then(|| f32::from_bits(value));
        let snapshot = if allow_snapshot
            && matches!(
                class,
                GlValueClass::MappedPointer | GlValueClass::CodePointer
            ) {
            Some(self.snapshot_memory(value, pointer_len))
        } else {
            None
        };
        GlRegisterSnapshot {
            name,
            value,
            class,
            float_value,
            snapshot,
        }
    }

    fn capture_stack_words(
        &mut self,
        stack_bytes: &[u8],
        pointer_len: usize,
    ) -> Vec<GlStackWordSnapshot> {
        let mut words = Vec::with_capacity(stack_bytes.len() / 4);
        for (index, chunk) in stack_bytes.chunks_exact(4).enumerate() {
            let value = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let class = self.classify_trace_value(value);
            let float_value = matches!(class, GlValueClass::Float).then(|| f32::from_bits(value));
            let snapshot = if matches!(
                class,
                GlValueClass::MappedPointer | GlValueClass::CodePointer
            ) {
                Some(self.snapshot_memory(value, pointer_len))
            } else {
                None
            };
            words.push(GlStackWordSnapshot {
                offset: index * 4,
                value,
                class,
                float_value,
                snapshot,
            });
        }
        words
    }

    fn classify_trace_value(&self, value: u32) -> GlValueClass {
        match self.memory_region(value) {
            GlMemoryRegion::WorkRam => GlValueClass::MappedPointer,
            GlMemoryRegion::Image | GlMemoryRegion::Trampoline => GlValueClass::CodePointer,
            GlMemoryRegion::Unmapped => {
                if value & 0x7f80_0000 != 0 {
                    GlValueClass::Float
                } else {
                    GlValueClass::Scalar
                }
            }
        }
    }

    fn memory_region(&self, value: u32) -> GlMemoryRegion {
        let work_end = WORK_RAM_BASE.saturating_add(WORK_RAM_SIZE as u32);
        let image_end = FILE_VMA_BASE.saturating_add(self.bus.image_len);
        if (WORK_RAM_BASE..work_end).contains(&value) {
            GlMemoryRegion::WorkRam
        } else if (FILE_VMA_BASE..image_end).contains(&value) {
            GlMemoryRegion::Image
        } else if (TRAMPOLINE_BASE..TRAMPOLINE_BASE.saturating_add(0x10000)).contains(&value) {
            GlMemoryRegion::Trampoline
        } else {
            GlMemoryRegion::Unmapped
        }
    }

    fn snapshot_memory(&mut self, addr: u32, len: usize) -> GlMemorySnapshot {
        self.snapshot_memory_with_bytes(addr, len).0
    }

    fn snapshot_memory_with_bytes(&mut self, addr: u32, len: usize) -> (GlMemorySnapshot, Vec<u8>) {
        let region = self.memory_region(addr);
        if addr == 0 || len == 0 {
            return (
                GlMemorySnapshot {
                    addr,
                    requested_len: len,
                    len: 0,
                    truncated: false,
                    region,
                    file_backing: None,
                    bytes_hex: String::new(),
                },
                Vec::new(),
            );
        }

        let mut bytes = Vec::with_capacity(len);
        for i in 0..len {
            match self.read_guest_u8(addr.wrapping_add(i as u32)) {
                Some(b) => bytes.push(b),
                None => break,
            }
        }
        let snapshot = GlMemorySnapshot {
            addr,
            requested_len: len,
            len: bytes.len(),
            truncated: bytes.len() < len,
            region,
            file_backing: self.file_backing_for_addr(addr),
            bytes_hex: hex_bytes(&bytes),
        };
        (snapshot, bytes)
    }

    fn file_backing_for_addr(&self, addr: u32) -> Option<GlFileBacking> {
        self.staged_files
            .values()
            .filter(|staged| {
                let end = staged.payload_addr.saturating_add(staged.len);
                (staged.payload_addr..end).contains(&addr)
            })
            .max_by_key(|staged| staged.generation)
            .map(|staged| GlFileBacking {
                path: self.describe_host_path(&staged.host_path),
                base_addr: staged.payload_addr,
                len: staged.len,
                offset: addr.saturating_sub(staged.payload_addr),
            })
    }

    fn describe_host_path(&self, host_path: &Path) -> String {
        if let Ok(rel) = host_path.strip_prefix(&self.metadata.bundle_dir) {
            return rel.display().to_string();
        }
        if let Ok(rel) = host_path.strip_prefix(self.metadata.bundle_dir.join(".clicky-saves")) {
            return format!(".clicky-saves/{}", rel.display());
        }
        host_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| host_path.display().to_string())
    }

    /// Scan guest work RAM for large contiguous non-zero regions and report
    /// any whose size is plausible for a framebuffer (e.g. 320*240*2 = 153600
    /// bytes for RGB565, or *4 = 307200 for RGBA8888). Also samples the first
    /// nonzero word of each large region so we can recognise texture data.
    pub fn scan_for_framebuffer(&self) {
        const BLOCK: usize = 256;
        let size = WORK_RAM_SIZE;
        let mut buf = vec![0u8; size];
        self.bus.work_ram.bulk_read(0, &mut buf);

        let is_nonzero = |win: &[u8]| win.iter().any(|&b| b != 0);
        let mut regions: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while i < size {
            // find next nonzero 256B block
            if !is_nonzero(&buf[i..i + BLOCK]) {
                i += BLOCK;
                continue;
            }
            let start = i;
            while i < size && is_nonzero(&buf[i..i + BLOCK]) {
                i += BLOCK;
            }
            regions.push((start, i - start));
        }

        // Only report regions >= ~1KB; sort by size desc.
        regions.retain(|&(_, len)| len >= 1024);
        regions.sort_by(|a, b| b.1.cmp(&a.1));

        info!(
            target: "EAPP",
            "work-ram nonzero regions (>=1KB): {} found; top 12 by size:",
            regions.len()
        );
        for &(off, len) in regions.iter().take(12) {
            let addr = WORK_RAM_BASE + off as u32;
            // sample first 4 nonzero words
            let mut sample = String::new();
            let mut taken = 0;
            let mut j = off;
            while j + 4 <= off + len && taken < 4 {
                let w = u32::from_le_bytes([buf[j], buf[j + 1], buf[j + 2], buf[j + 3]]);
                if w != 0 {
                    sample.push_str(&format!(" {:#010x}", w));
                    taken += 1;
                }
                j += 4;
            }
            // framebuffer-size hint
            let fb_hint = match len {
                153600 => " == 320*240*2 (RGB565)",
                307200 => " == 320*240*4 (RGBA8888)",
                76800 => " == 320*240*1 (A8)",
                _ => "",
            };
            info!(
                target: "EAPP",
                "  {:#010x} len={}{} sample:{}",
                addr, len, fb_hint, sample
            );
        }
    }

    pub fn step(&mut self) -> FatalMemResult<()> {
        if self.halted {
            return Ok(());
        }
        let pc = self.cpu.reg_get(self.cpu.mode(), reg::PC);
        self.record_pc(pc);
        if pc == BOOTSTRAP_RETURN_PC || (pc == 0 && self.bootstrap_phase != BootstrapPhase::Done) {
            self.handle_bootstrap_return();
            return Ok(());
        }
        if pc == GUEST_CALLBACK_RETURN_PC {
            self.handle_guest_callback_return();
            return Ok(());
        }
        if let Some(&import_idx) = self.trampoline_to_import.get(&pc) {
            self.handle_import(import_idx)?;
            return Ok(());
        }

        self.maybe_patch_guest_state(pc);
        if self.handle_guest_svc(pc) {
            return Ok(());
        }

        let mut mem = MemoryAdapter::new(&mut self.bus);
        self.cpu.step(&mut mem);
        if let Some((access, e)) = mem.exception.take() {
            let pc = self.cpu.reg_get(self.cpu.mode(), reg::PC);
            warn!(target: "EAPP", "recent pc trace: {}", self.format_recent_pcs());
            e.resolve(
                "EAPP",
                MemExceptionCtx {
                    pc,
                    access,
                    in_device: format!("eapp, {}", self.bus.probe(access.offset)),
                },
            )?;
        }
        Ok(())
    }

    fn handle_import(&mut self, import_idx: usize) -> FatalMemResult<()> {
        let import = self.imports[import_idx].clone();
        let pc = self.cpu.reg_get(self.cpu.mode(), reg::PC);
        self.record_pc(pc);
        let lr = self.cpu.reg_get(self.cpu.mode(), reg::LR);
        let args = [
            self.cpu.reg_get(self.cpu.mode(), 0),
            self.cpu.reg_get(self.cpu.mode(), 1),
            self.cpu.reg_get(self.cpu.mode(), 2),
            self.cpu.reg_get(self.cpu.mode(), 3),
        ];

        let key = (import.module.clone(), import.ordinal);
        *self.import_call_counts.entry(key.clone()).or_insert(0u64) += 1;
        *self.frame_import_counts.entry(key.clone()).or_insert(0u64) += 1;

        let in_gl_trace = self
            .gl_trace_frames
            .map(|(s, e)| self.frame_counter >= s && self.frame_counter <= e)
            .unwrap_or(false);
        if in_gl_trace && import.module == "OpenGLES" {
            info!(
                target: "EAPP_GL",
                "frame {} GL:{} lr={:#010x} r0={:#010x} r1={:#010x} r2={:#010x} r3={:#010x}",
                self.frame_counter,
                import.ordinal,
                lr,
                args[0],
                args[1],
                args[2],
                args[3]
            );
        }

        if self.logged_imports.insert(key.clone()) {
            info!(
                target: "EAPP_IMPORT",
                "{}:{} pc={:#010x} lr={:#010x} r0={:#010x} r1={:#010x} r2={:#010x} r3={:#010x}",
                import.module,
                import.ordinal,
                pc,
                lr,
                args[0],
                args[1],
                args[2],
                args[3]
            );
        } else {
            debug!(
                target: "EAPP_IMPORT",
                "{}:{} pc={:#010x} lr={:#010x}",
                import.module,
                import.ordinal,
                pc,
                lr
            );
        }

        let ret = match import.module.as_str() {
            "OpenGLES" => self.handle_open_gl_import(import.ordinal, args),
            "InputEvents" => self.handle_input_events_import(import.ordinal, args),
            "Settings" => self.handle_settings_import(import.ordinal, args),
            "Metadata" => 0,
            "miscTBD" => self.handle_misc_import(import.ordinal, args),
            "Audio" => 0,
            "AsyncFileIO" => self.handle_async_file_io_import(import.ordinal, args),
            other => {
                warn!(target: "EAPP_IMPORT", "unhandled module {}", other);
                self.fill_framebuffer(HLE_WARN_FRAMEBUFFER);
                0
            }
        };

        if import.module == "OpenGLES" {
            self.capture_open_gl_import(import.ordinal, pc, lr, args, ret);
        }

        self.cpu.reg_set(self.cpu.mode(), 0, ret);
        self.cpu.reg_set(self.cpu.mode(), reg::PC, lr & !1);
        Ok(())
    }

    fn handle_open_gl_import(&mut self, ordinal: u32, args: [u32; 4]) -> u32 {
        // Decode likely present/swap surface handles for diagnostic purposes.
        // Observed once-per-frame ordinals: 157, 158, 165. The handle in r0
        // (e.g. 0x0003f001) is logged with any guest memory it might point at.
        if matches!(ordinal, 157 | 158 | 165) {
            let handle = args[0];
            info!(
                target: "EAPP_GL",
                "GL:{} surface handle r0={:#010x} (r1={:#010x} r2={:#010x} r3={:#010x})",
                ordinal, handle, args[1], args[2], args[3]
            );
            self.decode_surface_handle(ordinal, handle);
            if self.gl_hle_enabled() {
                if let Some(lg) = self.live_gl.as_mut() {
                    lg.lifecycle_log.push(format!(
                        "frame={} ordinal={} handle={:#010x} (lifecycle role unconfirmed)",
                        self.frame_counter, ordinal, handle
                    ));
                }
            }
        }

        // Experimental live GL HLE path. When enabled, dispatch each observed
        // ordinal into persistent state and a software framebuffer. When
        // disabled, the legacy fill-color diagnostic path is used unchanged.
        if self.gl_hle_enabled() {
            self.handle_open_gl_hle(ordinal, args);
            return 0;
        }

        self.fill_framebuffer(HLE_OPENGL_FRAMEBUFFER);
        0
    }

    fn gl_hle_enabled(&self) -> bool {
        self.live_gl.is_some()
    }

    /// Read the experimental GL HLE env flags and construct live state only
    /// when `CLICKY_EXPERIMENTAL_GL_HLE=1`. Returns `None` (legacy path) when
    /// the flag is absent or not enabled, so default behavior is unchanged.
    fn maybe_init_live_gl() -> Option<LiveGlState> {
        let enabled = std::env::var_os("CLICKY_EXPERIMENTAL_GL_HLE")
            .map(|v| v.to_string_lossy() == "1")
            .unwrap_or(false);
        if !enabled {
            return None;
        }
        let present_vflip = std::env::var_os("CLICKY_GL_PRESENT_VFLIP")
            .and_then(|v| v.to_string_lossy().parse::<u32>().ok())
            .map(|n| n != 0)
            .unwrap_or(true);
        let gate_b = std::env::var_os("CLICKY_GL_GATE_B")
            .map(|v| v.to_string_lossy() == "1")
            .unwrap_or(false);
        let continuous = std::env::var_os("CLICKY_GL_LIVE_CONTINUOUS")
            .map(|v| v.to_string_lossy() == "1")
            .unwrap_or(false);
        let dump_frames = std::env::var_os("CLICKY_GL_DUMP_FRAMES")
            .and_then(|v| v.to_string_lossy().parse::<usize>().ok())
            .unwrap_or(0);
        info!(
            target: "EAPP_GL",
            "experimental GL HLE enabled: present_vflip={} gate_b={} continuous={} dump_frames={}",
            present_vflip, gate_b, continuous, dump_frames
        );
        let mut lg = LiveGlState::new(present_vflip, gate_b, continuous);
        lg.dump_remaining = dump_frames;
        Some(lg)
    }

    /// Experimental live GL HLE dispatch. Called for every OpenGLES import
    /// when the flag is enabled. Records state for the observed ordinals and
    /// drives the software framebuffer via `LiveGlState`.
    fn handle_open_gl_hle(&mut self, ordinal: u32, args: [u32; 4]) {
        let frame = self.frame_counter;
        let boundary = matches!(self.live_gl.as_ref(), Some(lg) if frame != lg.last_frame_counter);
        if boundary {
            // On the guest frame boundary, emit the previous frame's lifecycle
            // trace (evidence for begin/present detection) before resetting.
            if let Some(lg) = self.live_gl.as_mut() {
                let prev_frame = lg.last_frame_counter;
                let draws = lg.draw_count_in_frame;
                if let Some(summary) = lg.take_frame_trace_summary(prev_frame, draws) {
                    info!(target: "EAPP_GL", "{}", summary);
                    if lg.lifecycle_reports.len() < lg.lifecycle_report_budget {
                        lg.lifecycle_reports.push(summary);
                    }
                }
                lg.last_frame_counter = frame;
                lg.reset_for_frame();
            }
        }

        // Record this call in the current frame's lifecycle trace.
        let trace_handle = if matches!(ordinal, 157 | 158 | 165 | 159) {
            args[0]
        } else {
            0
        };
        if let Some(lg) = self.live_gl.as_mut() {
            lg.ordinal_trace.push((ordinal, trace_handle));
        }

        match ordinal {
            99 => self.live_handle_upload(args),
            137 => self.live_handle_array_def(args),
            40 => self.live_handle_enable_array(args),
            169 => self.live_handle_translate(args),
            159 => self.live_handle_bind_material(args),
            37 => self.live_handle_draw(args),
            // Candidate lifecycle from observed live ordering:
            // 158 always precedes all steady-state draws; 157 always follows.
            // Neutral names until exact ABI semantics are proven.
            158 => self.live_handle_candidate_begin(),
            157 => self.live_handle_candidate_present(),
            165 => {}
            // Draw-adjacent state ordinals; recorded by observation only.
            175 | 125 | 36 => {}
            // Upload prep/bind ordinals; not required for dimension-based
            // live texture selection.
            45 | 4 => {}
            _ => {
                // Unknown/unsupported ordinal; fail safe (no panic).
            }
        }
    }

    /// Candidate begin from observed live ordering: ordinal 158 is the first
    /// surface ordinal and always precedes steady-state draws. Neutral name;
    /// exact ABI semantics remain unproven.
    fn live_handle_candidate_begin(&mut self) {
        let continuous = self
            .live_gl
            .as_ref()
            .map(|lg| lg.continuous_capture)
            .unwrap_or(false);
        if !continuous {
            return; // one-shot diagnostic capture keeps its existing heuristic
        }
        if let Some(lg) = self.live_gl.as_mut() {
            let outcome = lg.begin_frame();
            if matches!(outcome, live_gl::BeginOutcome::DoubleBegin) {
                warn!(target: "EAPP_GL", "candidate_begin double-begin detected");
            }
        }
    }

    /// Candidate present from observed live ordering: ordinal 157 is the last
    /// surface ordinal and always follows steady-state draws. Neutral name;
    /// exact ABI semantics remain unproven.
    fn live_handle_candidate_present(&mut self) {
        let continuous = self
            .live_gl
            .as_ref()
            .map(|lg| lg.continuous_capture)
            .unwrap_or(false);
        if !continuous {
            return; // one-shot diagnostic capture keeps its existing heuristic
        }
        let completed = match self.live_gl.as_mut().and_then(|lg| lg.complete_frame()) {
            Some(frame) => frame,
            None => {
                warn!(target: "EAPP_GL", "candidate_present without active frame; discarded");
                return;
            }
        };

        // Continuous rendering publishes completed, non-empty 158→157 frames.
        // The old `== 4` gate was useful while validating the static splash,
        // but after runtime time starts advancing Tetris legitimately emits
        // 3-draw loading frames and later higher-draw menu frames. Rasterizer
        // behavior is unchanged; this only avoids pinning the window to the
        // last 4-draw splash frame.
        let should_present = completed.draw_count > 0;
        self.live_log_completed_frame(&completed, should_present);
        self.live_log_signature_detail(&completed);
        if should_present {
            self.capture_startup_completed_frame(&completed);
            self.live_dump_completed_frame();
            if self.live_gl.as_ref().map(|lg| lg.gate_b).unwrap_or(false) {
                self.live_present_completed_to_window();
            }
        }
    }

    /// Ordinal 99: copy guest pixel bytes immediately, validate bounds, and
    /// build a live texture. Supports RGB565/RGBA5551/RGBA4444/A8. Row order
    /// is preserved exactly as uploaded.
    fn live_handle_upload(&mut self, args: [u32; 4]) {
        let target = args[0];
        let width = args[3];
        let sp = self.cpu.reg_get(self.cpu.mode(), reg::SP);
        let height = self.read_guest_u32(sp).unwrap_or(0);
        let source_format = self.read_guest_u32(sp.wrapping_add(0x08)).unwrap_or(0);
        let pixel_type = self.read_guest_u32(sp.wrapping_add(0x0c)).unwrap_or(0);
        let source_ptr = self.read_guest_u32(sp.wrapping_add(0x10)).unwrap_or(0);

        if source_ptr == 0 || width == 0 || height == 0 {
            warn!(
                target: "EAPP_GL",
                "live_upload skipped: invalid dims/ptr target={:#x} {}x{} src={:#010x}",
                target, width, height, source_ptr
            );
            return;
        }
        let format = format_from_gl(source_format, pixel_type);
        if format.is_none() {
            warn!(
                target: "EAPP_GL",
                "live_upload skipped: unsupported format src_fmt={:#x} pix_type={:#x}",
                source_format, pixel_type
            );
            return;
        }
        let expected = pix_payload_size(format.unwrap(), width as usize, height as usize);
        let payload = match self.read_guest_bytes(source_ptr, expected) {
            Some(bytes) if bytes.len() == expected => bytes,
            _ => {
                warn!(
                    target: "EAPP_GL",
                    "live_upload skipped: short/invalid source ptr {:#010x} want={} bytes",
                    source_ptr, expected
                );
                return;
            }
        };

        let index = self.live_gl.as_ref().map(|l| l.uploads.len()).unwrap_or(0);
        let backing = self.file_backing_for_addr(source_ptr);
        let mut upload = LiveGlState::build_upload(
            index,
            target,
            width,
            height,
            source_format,
            pixel_type,
            source_ptr,
            &payload,
        );
        if let Some(backing) = backing {
            upload.source_file_offset = Some(source_ptr.saturating_sub(backing.base_addr));
            upload.source_file = Some(backing.path);
        }
        info!(
            target: "EAPP_GL",
            "live_upload idx={} {}x{} format={:?} src_fmt={:#x} pix_type={:#x} src_ptr={:#010x} bytes={} file={} file_off={}",
            index,
            width,
            height,
            upload.format,
            source_format,
            pixel_type,
            source_ptr,
            payload.len(),
            upload.source_file.as_deref().unwrap_or("<unknown>"),
            upload
                .source_file_offset
                .map(|off| format!("{}", off))
                .unwrap_or_else(|| "<unknown>".to_string())
        );
        if let Some(lg) = self.live_gl.as_mut() {
            lg.uploads.push(upload);
        }
    }

    /// Ordinal 137: record an array definition (direct args + sp+0, sp+4).
    /// Unknown array slots are preserved without semantic naming.
    fn live_handle_array_def(&mut self, args: [u32; 4]) {
        let array_index = args[0];
        let component_count = args[1];
        let format = args[2];
        let sp = self.cpu.reg_get(self.cpu.mode(), reg::SP);
        let stride = self.read_guest_u32(sp).unwrap_or(0);
        let guest_ptr = self.read_guest_u32(sp.wrapping_add(0x04)).unwrap_or(0);
        let valid = guest_ptr != 0 && component_count != 0;
        info!(
            target: "EAPP_GL",
            "live_array idx={} comps={} format={:#x} stride={} ptr={:#010x} valid={}",
            array_index, component_count, format, stride, guest_ptr, valid
        );
        if let Some(lg) = self.live_gl.as_mut() {
            let def = live_gl::LiveArrayDef {
                array_index,
                component_count,
                format,
                stride,
                guest_ptr,
                valid,
                material_epoch: lg.current_material_epoch,
            };
            lg.arrays.insert(array_index, def);
        }
    }

    /// Ordinal 40: enable/select an array by index (direct arg r0 only).
    fn live_handle_enable_array(&mut self, args: [u32; 4]) {
        let array_index = args[0];
        if let Some(lg) = self.live_gl.as_mut() {
            lg.enabled_arrays.insert(array_index);
        }
        debug!(target: "EAPP_GL", "live_enable_array idx={}", array_index);
    }

    /// Ordinal 169: accumulate translation (r1=tx, r2=ty as floats). Reset to
    /// zero after each confirmed draw (ordinal 37).
    fn live_handle_translate(&mut self, args: [u32; 4]) {
        let tx = f32::from_bits(args[1]);
        let ty = f32::from_bits(args[2]);
        if let Some(lg) = self.live_gl.as_mut() {
            lg.translation.0 += tx;
            lg.translation.1 += ty;
        }
    }

    /// Ordinal 159: record the small selector/handle (r0) and state blob
    /// pointer (r1). The exact handle-creation path remains unsolved.
    fn live_handle_bind_material(&mut self, args: [u32; 4]) {
        let handle = args[0];
        let state_ptr = args[1];
        if let Some(lg) = self.live_gl.as_mut() {
            lg.current_handle = handle;
            lg.current_state_ptr = state_ptr;
            lg.current_material_epoch = lg.current_material_epoch.wrapping_add(1);
        }
        info!(
            target: "EAPP_GL",
            "live_bind_material handle={:#x} state_ptr={:#010x}",
            handle, state_ptr
        );
    }

    /// Ordinal 37: confirmed DrawArrays(7, 0, 4). Read the current position
    /// and UV arrays, apply the accumulated translation, select the best live
    /// texture, and rasterize in guest draw order.
    fn live_handle_draw(&mut self, args: [u32; 4]) {
        let mode = args[0];
        let first = args[1] as usize;
        let count = args[2] as usize;
        if mode != live_gl::DRAW_MODE || count != 4 || first != 0 {
            warn!(
                target: "EAPP_GL",
                "live_draw skipped: unsupported mode={} first={} count={}",
                mode, first, count
            );
            self.live_finalize_draw(None);
            return;
        }

        if let Some(lg) = self.live_gl.as_mut() {
            if lg.continuous_capture && !lg.frame_active {
                warn!(target: "EAPP_GL", "draw outside active candidate frame; auto-beginning safely");
                lg.note_draw_outside_frame();
            }
        }

        let (
            handle,
            state_ptr,
            translation,
            pos_def,
            uv_def,
            pos_enabled,
            uv_enabled,
            enabled_arrays,
            draw_index,
            material_epoch,
        ) = {
            let lg = match self.live_gl.as_ref() {
                Some(lg) => lg,
                None => return,
            };
            let mut enabled_arrays: Vec<u32> = lg.enabled_arrays.iter().copied().collect();
            enabled_arrays.sort_unstable();
            (
                lg.current_handle,
                lg.current_state_ptr,
                lg.translation,
                lg.arrays.get(&0).cloned(),
                lg.arrays.get(&1).cloned(),
                lg.enabled_arrays.contains(&0),
                lg.enabled_arrays.contains(&1),
                enabled_arrays,
                lg.draws.len(),
                lg.current_material_epoch,
            )
        };
        let state_words = self.read_guest_words(state_ptr, 16);

        let positions = match self.live_decode_positions(&pos_def, pos_enabled, translation) {
            Some(p) => p,
            None => {
                let rec = live_gl::LiveDrawRecord {
                    draw_index,
                    handle,
                    state_ptr,
                    translation,
                    positions: [(0.0, 0.0); 4],
                    uvs: [(0.0, 0.0); 4],
                    has_uv: false,
                    solid_color: None,
                    position_array: pos_def.clone(),
                    uv_array: uv_def.clone(),
                    enabled_arrays: enabled_arrays.clone(),
                    state_words,
                    bounds: (0.0, 0.0, 0.0, 0.0),
                    coverage: 0,
                    selected_upload: None,
                    inferred_dim: None,
                    skipped_reason: Some("position array not enabled/valid/GL_FIXED".to_string()),
                };
                warn!(
                    target: "EAPP_GL",
                    "draw{} skipped: position array unusable handle={:#x}",
                    draw_index + 1, handle
                );
                self.live_finalize_draw(Some(rec));
                return;
            }
        };

        let (uvs, has_uv) = self
            .live_decode_uvs(&uv_def, uv_enabled, material_epoch)
            .unwrap_or(([(0.0, 0.0); 4], false));
        let solid_color = if has_uv || handle != 0x3 {
            None
        } else {
            self.live_decode_solid_color(&uv_def, uv_enabled, material_epoch)
        };

        let mut record = match self.live_gl.as_mut() {
            Some(lg) => lg.rasterize_draw(
                draw_index,
                handle,
                state_ptr,
                translation,
                positions,
                uvs,
                has_uv,
                solid_color,
            ),
            None => return,
        };
        record.position_array = pos_def.clone();
        record.uv_array = uv_def.clone();
        record.enabled_arrays = enabled_arrays;
        record.state_words = state_words;

        if let Some(reason) = record.skipped_reason.clone() {
            if let Some(lg) = self.live_gl.as_mut() {
                lg.note_skipped_draw(reason.clone());
            }
            warn!(
                target: "EAPP_GL",
                "draw{} skipped: {} handle={:#x}",
                draw_index + 1, reason, handle
            );
        } else if let Some(sel) = record.selected_upload {
            info!(
                target: "EAPP_GL",
                "draw{} rasterized handle={:#x} inferred_upload={} dim={:?} bounds=({:.1},{:.1})-({:.1},{:.1}) cov={}",
                draw_index + 1, handle, sel, record.inferred_dim, record.bounds.0, record.bounds.1,
                record.bounds.2, record.bounds.3, record.coverage
            );
        } else if let Some(color) = record.solid_color {
            info!(
                target: "EAPP_GL",
                "draw{} rasterized solid handle={:#x} color=rgba({},{},{},{}) bounds=({:.1},{:.1})-({:.1},{:.1}) cov={}",
                draw_index + 1, handle, color.r, color.g, color.b, color.a, record.bounds.0,
                record.bounds.1, record.bounds.2, record.bounds.3, record.coverage
            );
        }
        self.live_finalize_draw(Some(record));
    }

    /// Decode the 4-vertex position array (array 0, GL_FIXED) and apply the
    /// current translation. Returns None if the array is not usable.
    fn live_decode_positions(
        &mut self,
        def: &Option<live_gl::LiveArrayDef>,
        enabled: bool,
        translation: (f32, f32),
    ) -> Option<[(f32, f32); 4]> {
        let def = def.as_ref()?;
        if !enabled || !def.valid || def.format != live_gl::GL_FIXED || def.component_count < 2 {
            return None;
        }
        let pts = self.read_fixed_array(def.guest_ptr, def.component_count as usize, 4)?;
        Some([
            (pts[0].0 + translation.0, pts[0].1 + translation.1),
            (pts[1].0 + translation.0, pts[1].1 + translation.1),
            (pts[2].0 + translation.0, pts[2].1 + translation.1),
            (pts[3].0 + translation.0, pts[3].1 + translation.1),
        ])
    }

    /// Decode the 4-vertex UV array (array 1, GL_FIXED). Tetris also binds
    /// 4-component arrays in slot 1 for color/tint-like data; those are not
    /// texture coordinates. Epoch matching avoids reusing stale client arrays
    /// after a later material bind that only redefines array 0.
    fn live_decode_uvs(
        &mut self,
        def: &Option<live_gl::LiveArrayDef>,
        enabled: bool,
        material_epoch: u64,
    ) -> Option<([(f32, f32); 4], bool)> {
        let def = def.as_ref()?;
        if !enabled
            || !def.valid
            || def.material_epoch != material_epoch
            || def.format != live_gl::GL_FIXED
            || def.component_count != 2
        {
            return None;
        }
        let pts = self.read_fixed_array(def.guest_ptr, def.component_count as usize, 4)?;
        Some((
            [
                (pts[0].0, pts[0].1),
                (pts[1].0, pts[1].1),
                (pts[2].0, pts[2].1),
                (pts[3].0, pts[3].1),
            ],
            true,
        ))
    }

    /// Decode a 4-component GL_FIXED color/tint array as a conservative solid
    /// color. Tetris uses this shape for handle-3 fade/fill quads that do not
    /// provide a 2-component texcoord array. We average the four vertex colors;
    /// observed startup quads use uniform values.
    fn live_decode_solid_color(
        &mut self,
        def: &Option<live_gl::LiveArrayDef>,
        enabled: bool,
        material_epoch: u64,
    ) -> Option<Rgba8> {
        let def = def.as_ref()?;
        if !enabled
            || !def.valid
            || def.material_epoch != material_epoch
            || def.format != live_gl::GL_FIXED
            || def.component_count != 4
        {
            return None;
        }
        let stride = if def.stride == 0 {
            def.component_count as usize * 4
        } else {
            def.stride as usize
        };
        let mut acc = [0.0f32; 4];
        for vertex in 0..4usize {
            let base = def.guest_ptr.wrapping_add((vertex * stride) as u32);
            for (component, slot) in acc.iter_mut().enumerate() {
                let word = self.read_guest_u32(base.wrapping_add((component * 4) as u32))?;
                *slot += decode_fixed_16_16(word).clamp(0.0, 1.0);
            }
        }
        let to_u8 = |v: f32| ((v / 4.0) * 255.0).round().clamp(0.0, 255.0) as u8;
        Some(Rgba8::rgba(
            to_u8(acc[0]),
            to_u8(acc[1]),
            to_u8(acc[2]),
            to_u8(acc[3]),
        ))
    }

    /// Reset per-draw translation, increment the draw counter, and capture the
    /// first complete candidate frame (after the known steady-state four
    /// ordinal-37 draws) unless continuous capture is enabled.
    fn live_finalize_draw(&mut self, record: Option<live_gl::LiveDrawRecord>) {
        let should_capture;
        if let Some(lg) = self.live_gl.as_mut() {
            if let Some(rec) = record {
                lg.draws.push(rec);
            }
            lg.translation = (0.0, 0.0);
            lg.draw_count_in_frame += 1;
            if lg.continuous_capture {
                // Ongoing rendering is driven by the observed 158→157 frame
                // lifecycle, not by the one-shot repeated-signature heuristic.
                return;
            }
            let four_draws = lg.draw_count_in_frame == 4;
            if !four_draws {
                return;
            }
            // One-shot diagnostic capture only: first consecutive repeat of
            // the 4-draw handle signature is the stable frame.
            let current_handles: Vec<u32> = lg.draws.iter().map(|d| d.handle).collect();
            let steady = matches!(&lg.prev_draw_handles, Some(prev) if *prev == current_handles);
            lg.prev_draw_handles = Some(current_handles);
            should_capture = steady && !lg.captured_first_frame;
        } else {
            return;
        }
        if should_capture {
            self.live_capture_frame();
        }
    }

    /// Gate A: write internal + presented PPMs, print hashes, and run the
    /// structural comparison against the offline replay. Gate B: copy the
    /// presented buffer to the desktop render state when `CLICKY_GL_GATE_B=1`.
    fn live_capture_frame(&mut self) {
        let gate_b;
        {
            let lg = match self.live_gl.as_mut() {
                Some(lg) => lg,
                None => return,
            };
            lg.candidate_frames += 1;
            lg.captured_first_frame = true;
            let internal = lg.internal_hash();
            let presented = lg.presented_hash();
            let wrote = lg.write_diagnostic_ppms(
                std::path::Path::new("/tmp/tetris_live_gl_hle_internal.ppm"),
                std::path::Path::new("/tmp/tetris_live_gl_hle_presented.ppm"),
            );
            lg.presented = Some(lg.present());
            info!(
                target: "EAPP_GL",
                "live_capture frame={} draws={} internal_hash={:#018x} presented_hash={:#018x} present_vflip={} wrote_ppms={}",
                lg.last_frame_counter, lg.draw_count_in_frame, internal, presented, lg.present_vflip, wrote
            );
            gate_b = lg.gate_b;
        }

        self.live_compare_to_offline();

        // Gate B: present to the desktop window only when explicitly enabled.
        if gate_b {
            self.live_present_to_window();
        }
    }

    /// Bounded diagnostics for completed continuous frames (first 120 by
    /// default). Reports candidate begin/end ordering, hashes, repeated-frame
    /// count, skipped draws, and whether the frame was presented or discarded.
    fn live_log_completed_frame(&mut self, frame: &live_gl::CompletedFrame, presented: bool) {
        let Some(lg) = self.live_gl.as_ref() else {
            return;
        };
        if frame.index as usize > lg.diagnostics_budget {
            if lg.first_changed_frame == Some(frame.index) {
                info!(
                    target: "EAPP_GL",
                    "frame_hash_changed first_change_frame={} presented_hash={:#018x}",
                    frame.index,
                    frame.presented_hash
                );
            }
            return;
        }
        let begin_seq = lg
            .ordinal_trace
            .iter()
            .position(|(ord, _)| *ord == lg.candidate_begin_ordinal)
            .map(|idx| idx + 1);
        let present_seq = lg
            .ordinal_trace
            .iter()
            .rposition(|(ord, _)| *ord == lg.candidate_present_ordinal)
            .map(|idx| idx + 1);
        let signature = frame
            .handle_signature
            .iter()
            .map(|h| format!("{:#x}", h))
            .collect::<Vec<_>>()
            .join(",");
        info!(
            target: "EAPP_GL",
            "frame_diag idx={} begin={}@{:?} end={}@{:?} draws={} sig=[{}] internal={:#018x} presented={:#018x} repeated={} skipped={} unique_hashes={} status={}",
            frame.index,
            lg.candidate_begin_ordinal,
            begin_seq,
            lg.candidate_present_ordinal,
            present_seq,
            frame.draw_count,
            signature,
            frame.internal_hash,
            frame.presented_hash,
            lg.repeated_presented_count,
            frame.skipped_draws,
            lg.unique_presented_hashes.len(),
            if presented { "presented" } else { "discarded" }
        );
        if !lg.frame_anomalies.is_empty() && frame.index as usize <= 12 {
            info!(
                target: "EAPP_GL",
                "frame_diag anomalies_so_far={} latest={}",
                lg.frame_anomalies.len(),
                lg.frame_anomalies.last().unwrap()
            );
        }
        if lg.first_changed_frame == Some(frame.index) {
            info!(
                target: "EAPP_GL",
                "frame_hash_changed first_change_frame={} presented_hash={:#018x}",
                frame.index,
                frame.presented_hash
            );
        }
    }

    /// Emit a bounded, detailed draw report the first time a completed-frame
    /// signature appears. This is for visual-accuracy triage, not rendering.
    fn live_log_signature_detail(&mut self, frame: &live_gl::CompletedFrame) {
        let key = frame
            .handle_signature
            .iter()
            .map(|h| format!("{:#x}", h))
            .collect::<Vec<_>>()
            .join(",");
        let key = format!("draws={} [{}]", frame.draw_count, key);
        if !self.startup_signature_reports.insert(key.clone()) {
            return;
        }
        let Some(lg) = self.live_gl.as_ref() else {
            return;
        };
        info!(
            target: "EAPP_GL",
            "frame_signature_detail guest_frame={} completed_idx={} {} internal={:#018x} presented={:#018x}",
            self.frame_counter,
            frame.index,
            key,
            frame.internal_hash,
            frame.presented_hash
        );
        for draw in &lg.draws {
            let pos = array_summary(draw.position_array.as_ref());
            let uv = array_summary(draw.uv_array.as_ref());
            let upload = draw
                .selected_upload
                .and_then(|idx| lg.uploads.get(idx).map(|u| upload_summary(u)))
                .unwrap_or_else(|| "upload=<none>".to_string());
            let state_words = draw
                .state_words
                .iter()
                .take(12)
                .map(|w| format!("{:#010x}", w))
                .collect::<Vec<_>>()
                .join(",");
            let uvs = draw
                .uvs
                .iter()
                .map(|(u, v)| format!("({:.1},{:.1})", u, v))
                .collect::<Vec<_>>()
                .join(",");
            let color = draw
                .solid_color
                .map(|c| format!("solid=rgba({},{},{},{})", c.r, c.g, c.b, c.a))
                .unwrap_or_else(|| "solid=<none>".to_string());
            info!(
                target: "EAPP_GL",
                "draw_detail guest_frame={} draw={} handle={:#x} state_ptr={:#010x} enabled={:?} pos_arr={} uv_arr={} translation=({:.2},{:.2}) bounds=({:.1},{:.1})-({:.1},{:.1}) uvs=[{}] inferred_dim={:?} {} {} coverage={} status={} state_words=[{}]",
                self.frame_counter,
                draw.draw_index + 1,
                draw.handle,
                draw.state_ptr,
                draw.enabled_arrays,
                pos,
                uv,
                draw.translation.0,
                draw.translation.1,
                draw.bounds.0,
                draw.bounds.1,
                draw.bounds.2,
                draw.bounds.3,
                uvs,
                draw.inferred_dim,
                upload,
                color,
                draw.coverage,
                draw.skipped_reason.as_deref().unwrap_or("rasterized"),
                state_words
            );
        }
    }

    /// Optional startup capture (`CLICKY_STARTUP_CAPTURE_DIR=/tmp/...`). Writes
    /// a chronological TSV manifest for completed frames, and dumps PPMs for
    /// every presented framebuffer hash change plus periodic samples.
    fn capture_startup_completed_frame(&mut self, frame: &live_gl::CompletedFrame) {
        if !self.startup_capture.enabled {
            return;
        }
        if self.startup_capture.manifest_rows >= self.startup_capture.max_frames {
            return;
        }
        let host_us = self.host_start.elapsed().as_micros() as u64;
        let guest_time_current = self
            .read_guest_u32(self.app_object.wrapping_add(4))
            .unwrap_or(0);
        let guest_time_delta = self
            .read_guest_u32(self.app_object.wrapping_add(8))
            .unwrap_or(0);
        let hash_changed = self.startup_capture.last_hash != Some(frame.presented_hash);
        let periodic = self.frame_counter % self.startup_capture.periodic_interval == 0;
        let reason = if hash_changed {
            "hash_change"
        } else if periodic {
            "periodic"
        } else {
            ""
        };

        let mut output_path = String::new();
        if !reason.is_empty() && self.startup_capture.dump_count < self.startup_capture.max_dumps {
            let filename = format!(
                "startup_g{:06}_host{:012}_hash{:016x}.ppm",
                self.frame_counter, host_us, frame.presented_hash
            );
            let path = self.startup_capture.dir.join(filename);
            if let Some(fb) = self.live_gl.as_ref().map(|lg| lg.presented_buffer.clone()) {
                framebuffer_to_ppm(&path, &fb, live_gl::FB_WIDTH, live_gl::FB_HEIGHT);
                output_path = path.display().to_string();
                self.startup_capture.dump_count += 1;
            }
        }
        let handles = frame
            .handle_signature
            .iter()
            .map(|h| format!("{:#x}", h))
            .collect::<Vec<_>>()
            .join(",");
        let row = format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{:#018x}\t{:#018x}\t{}\t{}\n",
            self.frame_counter,
            host_us,
            guest_time_current,
            guest_time_delta,
            frame.draw_count,
            handles,
            frame.internal_hash,
            frame.presented_hash,
            reason,
            output_path
        );
        if let Ok(mut file) = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.startup_capture.manifest_path)
        {
            let _ = file.write_all(row.as_bytes());
        }
        self.startup_capture.manifest_rows += 1;
        self.startup_capture.last_hash = Some(frame.presented_hash);
    }

    /// Optional continuous frame dumping (`CLICKY_GL_DUMP_FRAMES=N`). Writes
    /// only the first N completed presented frames.
    fn live_dump_completed_frame(&mut self) {
        let (path, fb) = {
            let Some(lg) = self.live_gl.as_mut() else {
                return;
            };
            if lg.dump_remaining == 0 {
                return;
            }
            let path = format!("/tmp/tetris_live_frame_{:04}.ppm", lg.dump_counter);
            lg.dump_counter += 1;
            lg.dump_remaining -= 1;
            (path, lg.presented_buffer.clone())
        };
        framebuffer_to_ppm(
            std::path::Path::new(&path),
            &fb,
            live_gl::FB_WIDTH,
            live_gl::FB_HEIGHT,
        );
        info!(target: "EAPP_GL", "dumped_completed_frame path={}", path);
    }

    /// Gate B for continuous rendering: publish the most recent completed
    /// presented frame to the desktop window under the render-state mutex.
    fn live_present_completed_to_window(&mut self) {
        let presented = match self.live_gl.as_ref() {
            Some(lg) => lg.presented_buffer.clone(),
            None => return,
        };
        let mut frame = self.render_state.lock().unwrap();
        for (dst, src) in frame.iter_mut().zip(presented.iter()) {
            *dst = ((src.r as u32) << 16) | ((src.g as u32) << 8) | (src.b as u32);
        }
    }

    /// Print a bounded structural comparison between the live candidate and
    /// the known offline replay expectations. Hash equality is NOT required;
    /// only structural parity (draw count, bounds, formats, composition).
    fn live_compare_to_offline(&mut self) {
        let summary = self.live_gl.as_ref().map(|lg| {
            let mut lines = String::new();
            lines.push_str(&format!("\n  live draws observed: {}", lg.draws.len()));
            for d in &lg.draws {
                let dim = d
                    .inferred_dim
                    .map(|(w, h)| format!("{}x{}", w, h))
                    .unwrap_or_else(|| "?".into());
                let reason = d.skipped_reason.as_deref().unwrap_or("rasterized");
                lines.push_str(&format!(
                    "\n    draw{} handle={:#x} dim={} upload={:?} bounds=({:.0},{:.0})-({:.0},{:.0}) cov={} {}",
                    d.draw_index + 1, d.handle, dim, d.selected_upload, d.bounds.0, d.bounds.1,
                    d.bounds.2, d.bounds.3, d.coverage, reason
                ));
            }
            lines.push_str(&format!("\n  uploads: {}", lg.uploads.len()));
            for u in &lg.uploads {
                lines.push_str(&format!(
                    "\n    upload{} {}x{} format={:?} src={:#010x}",
                    u.index, u.width, u.height, u.format, u.source_ptr
                ));
            }
            lines.push_str("\n  offline reference: 4 draws; bg 320x240 RGB565, logo 250x162 RGBA4444, ea 50x50 RGBA5551, overlay handle 3 (unresolved)");
            lines
        });
        if let Some(s) = summary {
            info!(target: "EAPP_GL", "live_vs_offline summary:{}", s);
        }

        // Optional pixel-diff against the offline presented PPM if present.
        self.live_pixel_diff_against_offline();
    }

    /// Best-effort pixel-diff of the live frame against the offline replay
    /// reference PPM, if that artifact exists on disk. We compare the INTERNAL
    /// (unflipped) buffer against the offline draws-1-3 reference
    /// (`tetris_frame4_real_draws_1_3.ppm`), since both intentionally skip the
    /// unresolved handle-3 overlay. Exact hash equality is not required, but is
    /// expected here. Skipped silently if the reference is absent.
    fn live_pixel_diff_against_offline(&mut self) {
        let internal = match self.live_gl.as_ref() {
            Some(lg) => lg.framebuffer.clone(),
            None => return,
        };
        let reference = match read_ppm_p6(std::path::Path::new(
            "/tmp/tetris_frame4_real_draws_1_3.ppm",
        )) {
            Some(bytes) => bytes,
            None => {
                info!(
                    target: "EAPP_GL",
                    "pixel_diff skipped: no offline reference PPM at /tmp/tetris_frame4_real_draws_1_3.ppm"
                );
                return;
            }
        };
        if reference.len() != internal.len() {
            info!(
                target: "EAPP_GL",
                "pixel_diff skipped: size mismatch live={} ref={}",
                internal.len(),
                reference.len()
            );
            return;
        }
        let diff = internal
            .iter()
            .zip(reference.iter())
            .filter(|(a, b)| {
                // Reference PPM is opaque RGB (a=255); compare RGB only.
                a.r != b.r || a.g != b.g || a.b != b.b
            })
            .count();
        info!(
            target: "EAPP_GL",
            "pixel_diff_vs_offline(internal vs draws_1_3) differing_pixels={} / {} ({:.4}%)",
            diff,
            internal.len(),
            100.0 * diff as f32 / internal.len() as f32
        );
        if diff == 0 {
            info!(
                target: "EAPP_GL",
                "pixel_diff_vs_offline EXACT MATCH with offline draws_1_3 (unflipped)"
            );
        }
    }

    /// Gate B: copy the presented framebuffer to the shared desktop render
    /// state. Keeps the internal and presented buffers conceptually separate;
    /// the internal framebuffer is never mutated by presentation.
    fn live_present_to_window(&mut self) {
        let presented = match self.live_gl.as_ref() {
            Some(lg) => lg.presented.clone(),
            None => return,
        };
        let Some(presented) = presented else {
            return;
        };
        let mut frame = self.render_state.lock().unwrap();
        for (dst, src) in frame.iter_mut().zip(presented.iter()) {
            *dst = ((src.r as u32) << 16) | ((src.g as u32) << 8) | (src.b as u32);
        }
        info!(target: "EAPP_GL", "gate_b presented live framebuffer to eapp window");
    }

    /// Best-effort decode of a GL surface/swap handle. We do not yet know the
    /// exact encoding, so we try several interpretations and log each result.
    fn decode_surface_handle(&mut self, ordinal: u32, handle: u32) {
        // Interpretation 1: direct guest pointer into work RAM.
        if (WORK_RAM_BASE..WORK_RAM_BASE + WORK_RAM_SIZE as u32).contains(&handle) {
            info!(
                target: "EAPP_GL",
                "GL:{} handle {:#010x} is a work-ram pointer; first 8 words:",
                ordinal, handle
            );
            for off in (0..32).step_by(4) {
                let v = self
                    .read_guest_u32(handle.wrapping_add(off))
                    .unwrap_or(0xdeadbeef);
                info!(target: "EAPP_GL", "  +{:#04x}: {:#010x}", off, v);
            }
        }
        // Interpretation 2: small-integer name indexing a GL object table.
        // The high bits of 0x0003f001 may encode type; low bits an index.
        let idx = handle & 0xffff;
        let tag = handle >> 16;
        info!(
            target: "EAPP_GL",
            "GL:{} handle {:#010x} as name: tag={:#06x} idx={}",
            ordinal, handle, tag, idx
        );
    }

    fn handle_misc_import(&mut self, ordinal: u32, args: [u32; 4]) -> u32 {
        match ordinal {
            0 => {
                let len = args[0].max(args[1]).max(0x10);
                self.alloc_zeroed(len)
            }
            9 => {
                // Candidate monotonic tick API. Tetris calls this with r0
                // pointing at app_object+4 and app_object+8, then computes a
                // frame delta from the values stored there. The splash timeout
                // thresholds in the guest are 4_000_000 and 2_000_000, matching
                // microsecond units, so expose host monotonic microseconds.
                self.handle_misc9_time_api(args)
            }
            _ => 0,
        }
    }

    fn handle_input_events_import(&mut self, ordinal: u32, _args: [u32; 4]) -> u32 {
        let state = self.input_state.lock().unwrap().clone();
        match ordinal {
            // Heuristic: titles often poll a compact directional / button bitfield.
            0 => {
                let mut bits = 0u32;
                if state.up {
                    bits |= 1 << 0;
                }
                if state.down {
                    bits |= 1 << 1;
                }
                if state.left {
                    bits |= 1 << 2;
                }
                if state.right {
                    bits |= 1 << 3;
                }
                if state.action {
                    bits |= 1 << 4;
                }
                if state.menu {
                    bits |= 1 << 5;
                }
                bits
            }
            1 => self.alloc_zeroed(0x40),
            _ => 0,
        }
    }

    fn handle_settings_import(&mut self, ordinal: u32, _args: [u32; 4]) -> u32 {
        match ordinal {
            // Commonly-polled language / region / time-format values.
            0 => 0, // en_US-ish default
            1 => 0,
            2 => 0,
            _ => 0,
        }
    }

    fn handle_async_file_io_import(&mut self, ordinal: u32, args: [u32; 4]) -> u32 {
        let path = self
            .try_read_c_string(args[0], 256)
            .or_else(|| self.try_read_c_string(args[1], 256));
        if let Some(path) = path {
            info!(target: "EAPP_IMPORT", "AsyncFileIO:{} path={}", ordinal, path);
            self.fill_framebuffer(HLE_INFO_FRAMEBUFFER);

            if ordinal == 3 {
                let req = args[2];
                self.async_request_count = self.async_request_count.wrapping_add(1);
                if req != 0 {
                    self.async_pending_requests.insert(req);
                }
                self.dump_request_object(req);
                if let Some(host_path) = self.resolve_or_create_host_path(&path) {
                    // Request-object protocol (observed):
                    //   [req+0x14] = guest-provided destination buffer
                    //   [req+0x18] = expected byte count
                    //   [req+0x34] = completion callback pc
                    //   [req+0x38] = completion callback context
                    // We are the I/O layer, so we fill the guest's buffer.
                    let dest = self.read_guest_u32(req.wrapping_add(0x14)).unwrap_or(0);
                    let want = self.read_guest_u32(req.wrapping_add(0x18)).unwrap_or(0);
                    let callback_pc = self.read_guest_u32(req.wrapping_add(0x34)).unwrap_or(0);
                    let callback_ctx = self.read_guest_u32(req.wrapping_add(0x38)).unwrap_or(0);
                    if self.startup_progress.enabled {
                        info!(
                            target: "EAPP_PROGRESS",
                            "async_request frame={} count={} req={:#010x} dest={:#010x} want={} cb_pc={:#010x} cb_ctx={:#010x} path={}",
                            self.frame_counter,
                            self.async_request_count,
                            req,
                            dest,
                            want,
                            callback_pc,
                            callback_ctx,
                            path
                        );
                    }
                    match fs::read(&host_path) {
                        Ok(bytes) => {
                            let n = if want != 0 {
                                bytes.len().min(want as usize)
                            } else {
                                bytes.len()
                            };
                            let delivered = dest != 0 && self.write_guest_bytes(dest, &bytes[..n]);
                            if delivered {
                                info!(
                                    target: "EAPP_IMPORT",
                                    "AsyncFileIO:3 loaded {} ({} bytes) -> guest dest {:#010x}",
                                    host_path.display(),
                                    n,
                                    dest
                                );
                                self.staged_file_generation =
                                    self.staged_file_generation.wrapping_add(1);
                                self.staged_files.insert(
                                    req,
                                    StagedFile {
                                        generation: self.staged_file_generation,
                                        payload_addr: dest,
                                        len: n as u32,
                                        host_path: host_path.clone(),
                                    },
                                );
                            } else {
                                warn!(
                                    target: "EAPP_IMPORT",
                                    "AsyncFileIO:3 no dest buffer for {} (want {} bytes)",
                                    host_path.display(),
                                    want
                                );
                            }
                        }
                        Err(e) => {
                            warn!(
                                target: "EAPP_IMPORT",
                                "AsyncFileIO:3 read error for {}: {}",
                                host_path.display(),
                                e
                            );
                        }
                    }
                    info!(target: "EAPP_IMPORT", "AsyncFileIO:3 resolved={}", host_path.display());
                    if callback_pc != 0 {
                        self.async_callback_queued_count =
                            self.async_callback_queued_count.wrapping_add(1);
                        self.pending_guest_calls.push_back(PendingGuestCall {
                            pc: callback_pc,
                            arg0: req,
                            arg1: callback_ctx,
                        });
                        if self.startup_progress.enabled {
                            info!(
                                target: "EAPP_PROGRESS",
                                "async_callback_queued frame={} queued={} req={:#010x} cb_pc={:#010x} pending_async={}",
                                self.frame_counter,
                                self.async_callback_queued_count,
                                req,
                                callback_pc,
                                self.async_pending_requests.len()
                            );
                        }
                    } else {
                        self.async_pending_requests.remove(&req);
                    }
                    return 1;
                }
                self.async_pending_requests.remove(&req);
                warn!(target: "EAPP_IMPORT", "AsyncFileIO:3 missing host path {}", path);
                return 0;
            }

            return 1;
        }
        0
    }

    fn handle_misc9_time_api(&mut self, args: [u32; 4]) -> u32 {
        self.misc9_time_diag_count = self.misc9_time_diag_count.wrapping_add(1);
        let before = self.read_guest_u32(args[0]).unwrap_or(0xffff_ffff);
        let host_us = self.host_start.elapsed().as_micros() as u64;
        let guest_tick = host_us as u32;
        let wrote = args[0] != 0 && self.write_guest_u32(args[0], guest_tick);
        let after = self.read_guest_u32(args[0]).unwrap_or(0xffff_ffff);
        let guest_time_advances = self
            .misc9_last_pointed_value
            .map(|prev| prev != after)
            .unwrap_or(false);
        self.misc9_last_pointed_value = Some(after);
        let ret = args[0];
        let log_limit = std::env::var_os("CLICKY_EAPP_TIME_DIAG_LIMIT")
            .and_then(|v| v.to_string_lossy().parse::<u64>().ok())
            .unwrap_or(80);
        if self.startup_progress.enabled && self.misc9_time_diag_count <= log_limit {
            info!(
                target: "EAPP_PROGRESS",
                "time_api module=miscTBD ordinal=9 frame={} call={} args=[{:#010x},{:#010x},{:#010x},{:#010x}] pointed_before={:#010x} pointed_after={:#010x} ret={:#010x} host_us={} guest_time_advances={} writes_guest_time={}",
                self.frame_counter,
                self.misc9_time_diag_count,
                args[0],
                args[1],
                args[2],
                args[3],
                before,
                after,
                ret,
                host_us,
                guest_time_advances,
                wrote
            );
        }
        ret
    }

    fn maybe_log_startup_progress(&mut self) {
        if !self.startup_progress.enabled {
            return;
        }
        let frame = self.frame_counter;
        let fb_hash = self.render_state_hash();
        let hash_changed = self
            .startup_progress
            .last_framebuffer_hash
            .map(|prev| prev != fb_hash)
            .unwrap_or(false);
        if hash_changed && self.startup_progress.first_hash_change_frame.is_none() {
            self.startup_progress.first_hash_change_frame = Some(frame);
        }
        self.startup_progress.last_framebuffer_hash = Some(fb_hash);

        let should_log = frame <= 10
            || frame % self.startup_progress.interval == 0
            || hash_changed
            || self.startup_progress.logged < 10;
        if !should_log || self.startup_progress.logged >= self.startup_progress.max_logs {
            return;
        }
        self.startup_progress.logged += 1;

        let app_time_current = self
            .read_guest_u32(self.app_object.wrapping_add(4))
            .unwrap_or(0);
        let app_time_delta = self
            .read_guest_u32(self.app_object.wrapping_add(8))
            .unwrap_or(0);
        let frame_state = self.read_guest_u8(self.frame_context).unwrap_or(0xff);
        let frame_event_mask = self
            .read_guest_u32(self.frame_context.wrapping_add(0x20))
            .unwrap_or(0);
        let app_event_head = self
            .read_guest_u32(self.app_object.wrapping_add(0x30))
            .unwrap_or(0);
        let app_event_preview = self.preview_event_list(app_event_head, 4);
        let splash_base = 0x1802_56bc;
        let splash_phase = self.read_guest_u8(splash_base).unwrap_or(0xff);
        let splash_timeout_a = self
            .read_guest_u32(splash_base.wrapping_add(4))
            .unwrap_or(0);
        let splash_timeout_b = self
            .read_guest_u32(splash_base.wrapping_add(8))
            .unwrap_or(0);
        let splash_flags = self
            .read_guest_u32(splash_base.wrapping_add(0x14))
            .unwrap_or(0);
        let splash_time_a = self
            .read_guest_u32(splash_base.wrapping_add(0x18))
            .unwrap_or(0);
        let splash_time_b = self
            .read_guest_u32(splash_base.wrapping_add(0x1c))
            .unwrap_or(0);
        let splash_time_c = self
            .read_guest_u32(splash_base.wrapping_add(0x20))
            .unwrap_or(0);
        let import_summary = self.format_frame_import_counts(8);
        info!(
            target: "EAPP_PROGRESS",
            "startup_progress frame={} host_us={} fb_hash={:#018x} hash_changed={} first_hash_change={:?} app_time_current={} app_time_delta={} frame_state={} frame_event_mask={:#010x} app_event_head={:#010x} app_events=[{}] splash_phase={} splash_flags={:#010x} splash_timeout_a={} splash_timeout_b={} splash_times=[{},{},{}] async=req:{} queued:{} callbacks:{} pending:{} staged:{} imports=[{}]",
            frame,
            self.host_start.elapsed().as_micros() as u64,
            fb_hash,
            hash_changed,
            self.startup_progress.first_hash_change_frame,
            app_time_current,
            app_time_delta,
            frame_state,
            frame_event_mask,
            app_event_head,
            app_event_preview,
            splash_phase,
            splash_flags,
            splash_timeout_a,
            splash_timeout_b,
            splash_time_a,
            splash_time_b,
            splash_time_c,
            self.async_request_count,
            self.async_callback_queued_count,
            self.guest_callback_invocation_count,
            self.async_pending_requests.len(),
            self.staged_files.len(),
            import_summary
        );
    }

    fn render_state_hash(&self) -> u64 {
        let frame = self.render_state.lock().unwrap();
        let mut hasher = DefaultHasher::new();
        frame.hash(&mut hasher);
        hasher.finish()
    }

    fn read_guest_words(&mut self, addr: u32, count: usize) -> Vec<u32> {
        if addr == 0 {
            return Vec::new();
        }
        (0..count)
            .map(|i| {
                let a = addr.wrapping_add((i * 4) as u32);
                self.read_guest_u32(a).unwrap_or(0xffff_ffff)
            })
            .collect()
    }

    fn preview_words(&mut self, addr: u32, count: usize) -> String {
        self.read_guest_words(addr, count)
            .into_iter()
            .map(|w| format!("{:#010x}", w))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn preview_event_list(&mut self, mut head: u32, limit: usize) -> String {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for _ in 0..limit {
            if head == 0 || !seen.insert(head) {
                break;
            }
            let b0 = self.read_guest_u8(head).unwrap_or(0xff);
            let b1 = self.read_guest_u8(head.wrapping_add(1)).unwrap_or(0xff);
            let next = self.read_guest_u32(head.wrapping_add(8)).unwrap_or(0);
            out.push(format!(
                "{:#010x}:b0={} b1={} next={:#010x}",
                head, b0, b1, next
            ));
            head = next;
        }
        out.join("|")
    }

    fn format_frame_import_counts(&self, limit: usize) -> String {
        let mut counts: Vec<_> = self
            .frame_import_counts
            .iter()
            .filter(|((module, _), _)| module != "OpenGLES")
            .collect();
        counts.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        counts
            .into_iter()
            .take(limit)
            .map(|((module, ordinal), count)| format!("{}:{}={}", module, ordinal, count))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn fill_framebuffer(&mut self, color: u32) {
        let mut frame = self.render_state.lock().unwrap();
        frame.fill(color);
    }

    fn handle_bootstrap_return(&mut self) {
        match self.bootstrap_phase {
            BootstrapPhase::Entry => {
                let entry_r0 = self.cpu.reg_get(self.cpu.mode(), 0);
                let entry_r1 = self.cpu.reg_get(self.cpu.mode(), 1);
                let entry_r2 = self.cpu.reg_get(self.cpu.mode(), 2);
                let entry_r3 = self.cpu.reg_get(self.cpu.mode(), 3);
                let entry_r1_preview = self.preview_words(entry_r1, 12);
                self.app_object = self.alloc_zeroed(0x2000);
                self.frame_context = self.alloc_zeroed(0x80);
                info!(
                    target: "EAPP",
                    "bootstrap entry returned; entry_ret=[{:#010x},{:#010x},{:#010x},{:#010x}] entry_r1_words=[{}] app_object={:#010x} frame_context={:#010x} aux={:#010x}",
                    entry_r0,
                    entry_r1,
                    entry_r2,
                    entry_r3,
                    entry_r1_preview,
                    self.app_object,
                    self.frame_context,
                    self.header.aux_addr
                );
                self.bootstrap_phase = BootstrapPhase::Running;
                self.queue_next_frame();
                self.fill_framebuffer(HLE_INFO_FRAMEBUFFER);
            }
            BootstrapPhase::Running => {
                self.frame_counter = self.frame_counter.wrapping_add(1);
                if self.frame_counter == 1 || self.frame_counter % 600 == 0 {
                    info!(
                        target: "EAPP",
                        "frame {} returned r0={:#010x}",
                        self.frame_counter,
                        self.cpu.reg_get(self.cpu.mode(), 0)
                    );
                }
                self.maybe_log_startup_progress();
                self.frame_import_counts.clear();
                if !self.dispatch_pending_guest_call() {
                    self.queue_next_frame();
                }
            }
            BootstrapPhase::Done => {
                self.halted = true;
            }
        }
    }

    fn queue_next_frame(&mut self) {
        self.cpu.reg_set(self.cpu.mode(), 0, self.app_object);
        self.cpu.reg_set(self.cpu.mode(), 1, self.frame_context);
        self.cpu
            .reg_set(self.cpu.mode(), reg::LR, BOOTSTRAP_RETURN_PC);
        self.cpu
            .reg_set(self.cpu.mode(), reg::PC, self.header.aux_addr);
    }

    fn dispatch_pending_guest_call(&mut self) -> bool {
        if let Some(call) = self.pending_guest_calls.pop_front() {
            self.guest_callback_invocation_count =
                self.guest_callback_invocation_count.wrapping_add(1);
            self.async_pending_requests.remove(&call.arg0);
            if self.startup_progress.enabled {
                info!(
                    target: "EAPP_PROGRESS",
                    "callback_dispatch frame={} count={} pc={:#010x} arg0={:#010x} arg1={:#010x} pending_async={}",
                    self.frame_counter,
                    self.guest_callback_invocation_count,
                    call.pc,
                    call.arg0,
                    call.arg1,
                    self.async_pending_requests.len()
                );
            } else {
                debug!(
                    target: "EAPP",
                    "dispatching guest callback pc={:#010x} arg0={:#010x} arg1={:#010x}",
                    call.pc,
                    call.arg0,
                    call.arg1
                );
            }
            self.cpu.reg_set(self.cpu.mode(), 0, call.arg0);
            self.cpu.reg_set(self.cpu.mode(), 1, call.arg1);
            self.cpu
                .reg_set(self.cpu.mode(), reg::LR, GUEST_CALLBACK_RETURN_PC);
            self.cpu.reg_set(self.cpu.mode(), reg::PC, call.pc);
            return true;
        }
        false
    }

    fn handle_guest_callback_return(&mut self) {
        if !self.dispatch_pending_guest_call() {
            self.queue_next_frame();
        }
    }

    fn alloc_zeroed(&mut self, len: u32) -> u32 {
        let len = (len + 0xf) & !0xf;
        let addr = self.next_alloc;
        let end = addr.saturating_add(len);
        if end <= WORK_RAM_BASE + WORK_RAM_SIZE as u32 {
            self.next_alloc = end;
            addr
        } else {
            0
        }
    }

    fn read_guest_u8(&mut self, addr: u32) -> Option<u8> {
        self.bus.r8(addr).ok()
    }

    fn read_guest_u32(&mut self, addr: u32) -> Option<u32> {
        self.bus.r32(addr).ok()
    }

    /// Read `len` bytes of guest memory. Returns None on any unmapped byte so
    /// callers can log+skip malformed pointers without panicking.
    fn read_guest_bytes(&mut self, addr: u32, len: usize) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            out.push(self.bus.r8(addr.wrapping_add(i as u32)).ok()?);
        }
        Some(out)
    }

    /// Decode `vertex_count` vertices of `components` signed-16.16 fixed-point
    /// components each from guest memory. Returns the (x, y) of each vertex
    /// (extra components beyond 2 are ignored for 2D rasterization). Used for
    /// ordinal-137 position (4 comps) and UV (2 comps) arrays.
    fn read_fixed_array(
        &mut self,
        guest_ptr: u32,
        components: usize,
        vertex_count: usize,
    ) -> Option<Vec<(f32, f32)>> {
        let words = vertex_count * components;
        let bytes = self.read_guest_bytes(guest_ptr, words * 4)?;
        let mut pts = Vec::with_capacity(vertex_count);
        for v in 0..vertex_count {
            let base = (v * components) * 4;
            let x = decode_fixed_16_16(u32::from_le_bytes([
                bytes[base],
                bytes[base + 1],
                bytes[base + 2],
                bytes[base + 3],
            ]));
            let y = if components >= 2 {
                decode_fixed_16_16(u32::from_le_bytes([
                    bytes[base + 4],
                    bytes[base + 5],
                    bytes[base + 6],
                    bytes[base + 7],
                ]))
            } else {
                0.0
            };
            pts.push((x, y));
        }
        Some(pts)
    }

    fn write_guest_u32(&mut self, addr: u32, val: u32) -> bool {
        self.bus.w32(addr, val).is_ok()
    }

    fn write_guest_bytes(&mut self, addr: u32, bytes: &[u8]) -> bool {
        for (i, &b) in bytes.iter().enumerate() {
            if self.bus.w8(addr.wrapping_add(i as u32), b).is_err() {
                return false;
            }
        }
        true
    }

    /// Best-effort diagnostic dump of the AsyncFileIO request-object layout.
    /// Used to reverse-engineer where the guest expects file payload/length to
    /// be written. Logged once per request object address.
    fn dump_request_object(&mut self, req: u32) {
        if req == 0 || !self.dumped_requests.insert(req) {
            return;
        }
        let fields: [(usize, &str); 16] = [
            (0x00, "[0x00]"),
            (0x04, "[0x04] type"),
            (0x08, "[0x08]"),
            (0x0c, "[0x0c]"),
            (0x10, "[0x10]"),
            (0x14, "[0x14] arg2"),
            (0x18, "[0x18] arg3"),
            (0x1c, "[0x1c]"),
            (0x20, "[0x20]"),
            (0x24, "[0x24]"),
            (0x28, "[0x28]"),
            (0x2c, "[0x2c]"),
            (0x30, "[0x30]"),
            (0x34, "[0x34] cb_pc"),
            (0x38, "[0x38] cb_ctx"),
            (0x3c, "[0x3c]"),
        ];
        let mut rendered = String::new();
        for (off, label) in fields.iter() {
            let val = self
                .read_guest_u32(req.wrapping_add(*off as u32))
                .unwrap_or(0xdeadbeef);
            rendered.push_str(&format!("\n    {} {:#010x}", label, val));
        }
        info!(target: "EAPP", "request object @ {:#010x}:{}", req, rendered);
    }

    fn handle_guest_svc(&mut self, pc: u32) -> bool {
        if self.read_guest_u32(pc) != Some(0xef12_3456) {
            return false;
        }

        let call_num = self.cpu.reg_get(self.cpu.mode(), 0);
        let arg_ptr = self.cpu.reg_get(self.cpu.mode(), 1);
        match call_num {
            3 => {
                let ch = self.read_guest_u8(arg_ptr).unwrap_or_default();
                debug!(target: "EAPP", "svc: putchar {:?}", ch as char);
                self.cpu.reg_set(self.cpu.mode(), 0, ch as u32);
            }
            1 | 2 | 5 | 6 | 9 | 10 | 12 | 24 => {
                debug!(target: "EAPP", "svc: call {} arg_ptr={:#010x}", call_num, arg_ptr);
                self.cpu.reg_set(self.cpu.mode(), 0, 0);
            }
            other => {
                warn!(target: "EAPP", "unhandled guest svc call {} at pc={:#010x}", other, pc);
                self.cpu.reg_set(self.cpu.mode(), 0, 0);
            }
        }

        self.cpu
            .reg_set(self.cpu.mode(), reg::PC, pc.wrapping_add(4));
        true
    }

    fn maybe_patch_guest_state(&mut self, pc: u32) {
        if self.metadata.title != "66666" {
            return;
        }
        if !(0x18013d4c..=0x18014020).contains(&pc) {
            return;
        }

        let owner = match self.cpu.reg_get(self.cpu.mode(), 9) {
            0 => return,
            addr => addr,
        };
        let array = match self.read_guest_u32(owner.wrapping_add(8)) {
            Some(0) | None => return,
            Some(addr) => addr,
        };

        let mut patched = 0;
        for idx in 20..=37u32 {
            let slot_addr = array.wrapping_add(idx * 4);
            if self.read_guest_u32(slot_addr).unwrap_or(0) != 0 {
                continue;
            }
            let entry = self.alloc_zeroed(0x20);
            let payload = self.alloc_zeroed(0x200);
            if entry == 0 || payload == 0 {
                break;
            }
            if !self.write_guest_u32(slot_addr, entry) {
                break;
            }
            let _ = self.write_guest_u32(entry.wrapping_add(8), payload);
            patched += 1;
        }

        if patched > 0 {
            warn!(
                target: "EAPP",
                "patched {} placeholder Tetris resource slots at owner={:#010x} array={:#010x}",
                patched,
                owner,
                array
            );
        }
    }

    fn resolve_bundle_path(&self, path: &str) -> Option<PathBuf> {
        let normalized = path.trim_start_matches('/').trim_start_matches('\\');
        for candidate in [path, normalized] {
            if candidate.is_empty() {
                continue;
            }
            let direct = self.metadata.bundle_dir.join(candidate);
            if direct.exists() {
                return Some(direct);
            }
            let resources = self.metadata.bundle_dir.join("Resources").join(candidate);
            if resources.exists() {
                return Some(resources);
            }
        }
        None
    }

    fn resolve_or_create_host_path(&self, path: &str) -> Option<PathBuf> {
        if let Some(found) = self.resolve_bundle_path(path) {
            return Some(found);
        }

        let normalized = path.trim_start_matches('/').trim_start_matches('\\');
        if normalized.is_empty() {
            return None;
        }

        let writable = self
            .metadata
            .bundle_dir
            .join(".clicky-saves")
            .join(normalized);
        if let Some(parent) = writable.parent() {
            fs::create_dir_all(parent).ok()?;
        }
        if !writable.exists() {
            fs::write(&writable, []).ok()?;
        }
        Some(writable)
    }

    fn try_read_c_string(&mut self, addr: u32, max_len: usize) -> Option<String> {
        if addr == 0 {
            return None;
        }
        let mut bytes = Vec::new();
        for i in 0..max_len {
            let b = self.bus.r8(addr.wrapping_add(i as u32)).ok()?;
            if b == 0 {
                break;
            }
            if !(0x20..=0x7e).contains(&b) && b != b'/' && b != b'\\' && b != b'_' && b != b'.' {
                return None;
            }
            bytes.push(b);
        }
        if bytes.is_empty() {
            return None;
        }
        String::from_utf8(bytes).ok()
    }

    fn record_pc(&mut self, pc: u32) {
        if self.recent_pcs.back().copied() == Some(pc) {
            return;
        }
        if self.recent_pcs.len() == RECENT_PC_LIMIT {
            self.recent_pcs.pop_front();
        }
        self.recent_pcs.push_back(pc);
    }

    fn format_recent_pcs(&self) -> String {
        self.recent_pcs
            .iter()
            .map(|pc| format!("{:#010x}", pc))
            .collect::<Vec<_>>()
            .join(" -> ")
    }
}

fn array_summary(def: Option<&live_gl::LiveArrayDef>) -> String {
    match def {
        Some(def) => format!(
            "idx={} comps={} fmt={:#x} stride={} ptr={:#010x} valid={} epoch={}",
            def.array_index,
            def.component_count,
            def.format,
            def.stride,
            def.guest_ptr,
            def.valid,
            def.material_epoch
        ),
        None => "<none>".to_string(),
    }
}

fn upload_summary(upload: &live_gl::LiveGlUpload) -> String {
    format!(
        "upload={} file={} file_off={} dim={}x{} format={:?} src_fmt={:#x} pix_type={:#x}",
        upload.index,
        upload.source_file.as_deref().unwrap_or("<unknown>"),
        upload
            .source_file_offset
            .map(|off| off.to_string())
            .unwrap_or_else(|| "<unknown>".to_string()),
        upload.width,
        upload.height,
        upload.format,
        upload.source_format,
        upload.pixel_type
    )
}

impl TakeControls for Eapp {
    type Controls = EappBinds;

    fn take_controls(&mut self) -> Option<Self::Controls> {
        self.controls.take()
    }
}

impl EappImage {
    pub fn load(metadata: EappMetadata) -> Result<EappImage, EappBuildError> {
        let image = fs::read(&metadata.executable_path)?;
        let header = parse_eapp_header(&image)?;
        let imports = parse_import_modules(&image, header.imports_addr)?;
        Ok(EappImage {
            metadata,
            header,
            imports,
            image,
        })
    }
}

impl Device for EappBus {
    fn kind(&self) -> &'static str {
        "EappBus"
    }

    fn probe(&self, offset: u32) -> Probe {
        match offset {
            FILE_VMA_BASE..=u32::MAX if offset - FILE_VMA_BASE < self.image_len => Probe::Device {
                kind: "Ram",
                label: Some("eapp-image"),
                next: Box::new(self.image.probe(offset - FILE_VMA_BASE)),
            },
            WORK_RAM_BASE..=u32::MAX if offset - WORK_RAM_BASE < WORK_RAM_SIZE as u32 => {
                Probe::Device {
                    kind: "Ram",
                    label: Some("eapp-work"),
                    next: Box::new(self.work_ram.probe(offset - WORK_RAM_BASE)),
                }
            }
            _ => Probe::Unmapped,
        }
    }
}

impl Memory for EappBus {
    fn r32(&mut self, offset: u32) -> MemResult<u32> {
        match offset {
            FILE_VMA_BASE..=u32::MAX if offset - FILE_VMA_BASE < self.image_len => {
                self.image.r32(offset - FILE_VMA_BASE)
            }
            WORK_RAM_BASE..=u32::MAX if offset - WORK_RAM_BASE < WORK_RAM_SIZE as u32 => {
                self.work_ram.r32(offset - WORK_RAM_BASE)
            }
            _ => Err(MemException::Unexpected),
        }
    }

    fn w32(&mut self, offset: u32, val: u32) -> MemResult<()> {
        match offset {
            FILE_VMA_BASE..=u32::MAX if offset - FILE_VMA_BASE < self.image_len => {
                self.image.w32(offset - FILE_VMA_BASE, val)
            }
            WORK_RAM_BASE..=u32::MAX if offset - WORK_RAM_BASE < WORK_RAM_SIZE as u32 => {
                self.work_ram.w32(offset - WORK_RAM_BASE, val)
            }
            _ => Err(MemException::Unexpected),
        }
    }

    fn r8(&mut self, offset: u32) -> MemResult<u8> {
        match offset {
            FILE_VMA_BASE..=u32::MAX if offset - FILE_VMA_BASE < self.image_len => {
                self.image.r8(offset - FILE_VMA_BASE)
            }
            WORK_RAM_BASE..=u32::MAX if offset - WORK_RAM_BASE < WORK_RAM_SIZE as u32 => {
                self.work_ram.r8(offset - WORK_RAM_BASE)
            }
            _ => Err(MemException::Unexpected),
        }
    }

    fn r16(&mut self, offset: u32) -> MemResult<u16> {
        match offset {
            FILE_VMA_BASE..=u32::MAX if offset - FILE_VMA_BASE < self.image_len => {
                self.image.r16(offset - FILE_VMA_BASE)
            }
            WORK_RAM_BASE..=u32::MAX if offset - WORK_RAM_BASE < WORK_RAM_SIZE as u32 => {
                self.work_ram.r16(offset - WORK_RAM_BASE)
            }
            _ => Err(MemException::Unexpected),
        }
    }

    fn w8(&mut self, offset: u32, val: u8) -> MemResult<()> {
        match offset {
            FILE_VMA_BASE..=u32::MAX if offset - FILE_VMA_BASE < self.image_len => {
                self.image.w8(offset - FILE_VMA_BASE, val)
            }
            WORK_RAM_BASE..=u32::MAX if offset - WORK_RAM_BASE < WORK_RAM_SIZE as u32 => {
                self.work_ram.w8(offset - WORK_RAM_BASE, val)
            }
            _ => Err(MemException::Unexpected),
        }
    }

    fn w16(&mut self, offset: u32, val: u16) -> MemResult<()> {
        match offset {
            FILE_VMA_BASE..=u32::MAX if offset - FILE_VMA_BASE < self.image_len => {
                self.image.w16(offset - FILE_VMA_BASE, val)
            }
            WORK_RAM_BASE..=u32::MAX if offset - WORK_RAM_BASE < WORK_RAM_SIZE as u32 => {
                self.work_ram.w16(offset - WORK_RAM_BASE, val)
            }
            _ => Err(MemException::Unexpected),
        }
    }

    fn x16(&mut self, offset: u32) -> MemResult<u16> {
        self.r16(offset)
    }

    fn x32(&mut self, offset: u32) -> MemResult<u32> {
        self.r32(offset)
    }
}

/// Best-effort reader for a binary P6 PPM (used by the optional live-vs-offline
/// pixel diff). Returns the decoded RGBA8 pixel buffer or None on any parse
/// error. Only supports the exact format written by `framebuffer_to_ppm`.
fn read_ppm_p6(path: &std::path::Path) -> Option<Vec<Rgba8>> {
    let bytes = std::fs::read(path).ok()?;
    if !bytes.starts_with(b"P6") {
        return None;
    }
    let mut idx = 2usize;
    let mut fields = Vec::new();
    while fields.len() < 3 {
        // skip whitespace
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx < bytes.len() && bytes[idx] == b'#' {
            while idx < bytes.len() && bytes[idx] != b'\n' {
                idx += 1;
            }
            continue;
        }
        let start = idx;
        while idx < bytes.len() && !bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        let tok = std::str::from_utf8(&bytes[start..idx]).ok()?;
        fields.push(tok.parse::<u32>().ok()?);
        if fields.len() == 3 {
            // skip single whitespace after maxval
            idx += 1;
            break;
        }
    }
    let width = fields[0] as usize;
    let height = fields[1] as usize;
    let _maxval = fields[2];
    let payload = &bytes[idx..];
    let need = width * height * 3;
    if payload.len() < need {
        return None;
    }
    let mut out = Vec::with_capacity(width * height);
    for px in payload[..need].chunks_exact(3) {
        out.push(Rgba8::rgba(px[0], px[1], px[2], 255));
    }
    Some(out)
}

fn make_controls(input_state: Arc<Mutex<EappInputState>>) -> EappBinds {
    let mut controls = EappBinds::default();

    macro_rules! bind_key {
        ($key:expr, $field:ident) => {
            let state = Arc::clone(&input_state);
            controls.keys.insert(
                $key,
                Box::new(move |pressed| {
                    state.lock().unwrap().$field = pressed;
                }),
            );
        };
    }

    bind_key!(EappKey::Up, up);
    bind_key!(EappKey::Down, down);
    bind_key!(EappKey::Left, left);
    bind_key!(EappKey::Right, right);
    bind_key!(EappKey::Action, action);
    bind_key!(EappKey::Menu, menu);

    let state = Arc::clone(&input_state);
    controls.wheel = Some(Box::new(move |(_dx, dy)| {
        state.lock().unwrap().wheel_delta += dy;
    }));

    controls
}

fn find_game_executable(bundle_dir: &Path) -> Result<PathBuf, EappBuildError> {
    let exe_dir = bundle_dir.join("Executables");
    let mut bins = fs::read_dir(&exe_dir)
        .map_err(|_| EappBuildError::MissingExecutable(bundle_dir.display().to_string()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().map(|ext| ext == "bin").unwrap_or(false))
        .collect::<Vec<_>>();
    bins.sort();
    bins.into_iter()
        .next()
        .ok_or_else(|| EappBuildError::MissingExecutable(bundle_dir.display().to_string()))
}

fn parse_eapp_header(image: &[u8]) -> Result<EappHeader, EappBuildError> {
    if image.len() < EAPP_HEADER_SIZE {
        return Err(EappBuildError::InvalidImage(
            "file too small for eapp header".into(),
        ));
    }
    if &image[0..4] != b"eapp" {
        return Err(EappBuildError::InvalidImage("missing eapp magic".into()));
    }

    let load_addr_guess = read_u32_at(image, 0x04)?;
    let format_version = read_u32_at(image, 0x08)?;
    let header_size = read_u32_at(image, 0x0c)?;
    let imports_addr = read_u32_at(image, 0x10)?;
    let entry_addr = read_u32_at(image, 0x14)?;
    let init_addr = read_u32_at(image, 0x18)?;
    let aux_addr = read_u32_at(image, 0x24)?;

    Ok(EappHeader {
        load_addr_guess,
        format_version,
        header_size,
        imports_addr,
        entry_addr,
        init_addr,
        aux_addr,
    })
}

fn parse_import_modules(
    image: &[u8],
    mut name_addr: u32,
) -> Result<Vec<EappImportModule>, EappBuildError> {
    let mut modules = Vec::new();
    let mut seen = HashSet::new();

    while name_addr != 0 {
        if !seen.insert(name_addr) {
            return Err(EappBuildError::InvalidImage(format!(
                "import descriptor loop at {:#010x}",
                name_addr
            )));
        }

        let name_offset = vma_to_offset(name_addr)? as usize;
        let name_bytes = image
            .get(name_offset..name_offset + IMPORT_NAME_LEN)
            .ok_or_else(|| EappBuildError::InvalidImage("truncated import name".into()))?;
        let name = c_string(name_bytes)?;
        let count = read_u32_at(image, name_offset + IMPORT_COUNT_OFFSET)?;
        let next_addr = read_u32_at(image, name_offset + IMPORT_NEXT_OFFSET)?;
        let stubs_addr = name_addr + IMPORT_STUBS_OFFSET as u32;
        let literals_addr = stubs_addr + count * 4;

        if name == IMPORT_SENTINEL_NAME {
            break;
        }

        modules.push(EappImportModule {
            name_addr,
            name,
            count,
            next_addr,
            stubs_addr,
            literals_addr,
        });
        name_addr = next_addr;
    }

    Ok(modules)
}

fn c_string(bytes: &[u8]) -> Result<String, EappBuildError> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let slice = &bytes[..end];
    String::from_utf8(slice.to_vec())
        .map_err(|_| EappBuildError::InvalidImage("non-utf8 import name".into()))
}

fn read_u32_at(image: &[u8], offset: usize) -> Result<u32, EappBuildError> {
    let bytes = image
        .get(offset..offset + 4)
        .ok_or_else(|| EappBuildError::InvalidImage(format!("truncated u32 at {:#x}", offset)))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn vma_to_offset(addr: u32) -> Result<u32, EappBuildError> {
    addr.checked_sub(FILE_VMA_BASE).ok_or_else(|| {
        EappBuildError::InvalidImage(format!("address {:#010x} is outside file VMA", addr))
    })
}
