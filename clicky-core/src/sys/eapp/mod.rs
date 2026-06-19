use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use armv4t_emu::{reg, Cpu, Mode as ArmMode};
use thiserror::Error;

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
    halted: bool,
}

#[derive(Debug, Clone)]
struct StagedFile {
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
            halted: false,
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

        self.cpu.reg_set(self.cpu.mode(), 0, ret);
        self.cpu.reg_set(self.cpu.mode(), reg::PC, lr & !1);
        Ok(())
    }

    fn handle_open_gl_import(&mut self, _ordinal: u32, _args: [u32; 4]) -> u32 {
        self.fill_framebuffer(HLE_OPENGL_FRAMEBUFFER);
        0
    }

    fn handle_misc_import(&mut self, ordinal: u32, args: [u32; 4]) -> u32 {
        match ordinal {
            0 => {
                let len = args[0].max(args[1]).max(0x10);
                self.alloc_zeroed(len)
            }
            9 => args[0],
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
                                self.staged_files.insert(
                                    req,
                                    StagedFile {
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
                    if let Some(callback_pc) = self.read_guest_u32(req.wrapping_add(0x34)) {
                        let callback_ctx = self.read_guest_u32(req.wrapping_add(0x38)).unwrap_or(0);
                        if callback_pc != 0 {
                            self.pending_guest_calls.push_back(PendingGuestCall {
                                pc: callback_pc,
                                arg0: req,
                                arg1: callback_ctx,
                            });
                        }
                    }
                    return 1;
                }
                warn!(target: "EAPP_IMPORT", "AsyncFileIO:3 missing host path {}", path);
                return 0;
            }

            return 1;
        }
        0
    }

    fn fill_framebuffer(&mut self, color: u32) {
        let mut frame = self.render_state.lock().unwrap();
        frame.fill(color);
    }

    fn handle_bootstrap_return(&mut self) {
        match self.bootstrap_phase {
            BootstrapPhase::Entry => {
                self.app_object = self.alloc_zeroed(0x2000);
                self.frame_context = self.alloc_zeroed(0x80);
                info!(
                    target: "EAPP",
                    "bootstrap entry returned; app_object={:#010x} frame_context={:#010x} aux={:#010x}",
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
            debug!(
                target: "EAPP",
                "dispatching guest callback pc={:#010x} arg0={:#010x} arg1={:#010x}",
                call.pc,
                call.arg0,
                call.arg1
            );
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
