mod command;
mod common;

mod utils;
#[cfg(feature = "vulkan")]
mod vulkan;

use utils::PeekableReceiver;
#[cfg(feature = "vulkan")]
use vulkan as backend;

#[cfg(not(feature = "vulkan"))]
mod dummy_render;

#[cfg(not(feature = "vulkan"))]
use dummy_render as backend;

use crate::memory::{interrupts::InterruptRequester, BusLine, Result};
use command::{instantiate_gp0_command, Gp0CmdType, Gp0Command};

use core::fmt;
#[cfg(feature = "vulkan")]
use std::thread::JoinHandle;

use std::{
    ops::Range,
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc, Arc,
    },
};

use common::{DrawingTextureParams, DrawingVertex};

use backend::StandardCommandBufferAllocator;
pub use backend::{Device, GpuFuture, Image, Queue};

#[cfg(feature = "vulkan")]
use backend::{AutoCommandBufferBuilder, BlitImageInfo, CommandBufferUsage, Filter};

bitflags::bitflags! {
    #[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
    struct GpuStat: u32 {
        const TEXTURE_PAGE_X_BASE      = 0b00000000000000000000000000001111;
        const TEXTURE_PAGE_Y_BASE      = 0b00000000000000000000000000010000;
        const SEMI_TRASPARENCY         = 0b00000000000000000000000001100000;
        const TEXTURE_PAGE_COLORS      = 0b00000000000000000000000110000000;
        const DITHER_ENABLED           = 0b00000000000000000000001000000000;
        const DRAWING_TO_DISPLAY_AREA  = 0b00000000000000000000010000000000;
        const DRAWING_MASK_BIT         = 0b00000000000000000000100000000000;
        const NO_DRAW_ON_MASK          = 0b00000000000000000001000000000000;
        const INTERLACE_FIELD          = 0b00000000000000000010000000000000;
        const REVERSE_FLAG             = 0b00000000000000000100000000000000;
        const DISABLE_TEXTURE          = 0b00000000000000001000000000000000;
        const HORIZONTAL_RESOLUTION2   = 0b00000000000000010000000000000000;
        const HORIZONTAL_RESOLUTION1   = 0b00000000000001100000000000000000;
        const VERTICAL_RESOLUTION      = 0b00000000000010000000000000000000;
        const VIDEO_MODE               = 0b00000000000100000000000000000000;
        const DISPLAY_AREA_COLOR_DEPTH = 0b00000000001000000000000000000000;
        const VERTICAL_INTERLACE       = 0b00000000010000000000000000000000;
        const DISPLAY_DISABLED         = 0b00000000100000000000000000000000;
        const INTERRUPT_REQUEST        = 0b00000001000000000000000000000000;
        const DMA_DATA_REQUEST         = 0b00000010000000000000000000000000;
        const READY_FOR_CMD_RECV       = 0b00000100000000000000000000000000;
        const READY_FOR_TO_SEND_VRAM   = 0b00001000000000000000000000000000;
        const READY_FOR_DMA_RECV       = 0b00010000000000000000000000000000;
        const DMA_DIRECTION            = 0b01100000000000000000000000000000;
        const INTERLACE_ODD_EVEN_LINES = 0b10000000000000000000000000000000;
    }
}

#[cfg_attr(not(feature = "vulkan"), allow(dead_code))]
impl GpuStat {
    fn _texture_page_coords(&self) -> (u32, u32) {
        let x = (self.bits() & Self::TEXTURE_PAGE_X_BASE.bits()) * 64;
        let y = (self.intersects(Self::TEXTURE_PAGE_Y_BASE) as u32) * 256;

        (x, y)
    }

    fn horizontal_resolution(&self) -> u32 {
        if self.intersects(Self::HORIZONTAL_RESOLUTION2) {
            368
        } else {
            // HORIZONTAL_RESOLUTION1 is two bits:
            // 0  (if set, Add 64 to the 256 original resoltion)
            // 1  (if set, Multiply the current resolution by 2)
            //
            // result:
            // 0: 256
            // 1: 320
            // 2: 512
            // 3: 640
            let resolution_multiplier = (self.bits() & Self::HORIZONTAL_RESOLUTION1.bits()) >> 17;
            let resoltion = 0x100 | ((resolution_multiplier & 1) << 6);
            resoltion << (resolution_multiplier >> 1)
        }
    }

    // divider to get the dots per scanline
    // dots_per_line = cycles_per_line / divider
    fn horizontal_dots_divider(&self) -> u32 {
        if self.intersects(Self::HORIZONTAL_RESOLUTION2) {
            7
        } else {
            // we want the result to be:
            // 0: 10
            // 1: 8
            // 2: 5
            // 3: 4
            //
            // The second two numbers are half the first two, so we can use the
            // second bit to divide by 2.
            let resolution_bits = (self.bits() & Self::HORIZONTAL_RESOLUTION1.bits()) >> 17;

            // 4 is the base, we add 1 if the first bit is cleared, to get 5 and 10
            let base = 4 | ((resolution_bits & 1) ^ 1);
            // multiply by 2 if the second bit is cleared
            base << ((resolution_bits >> 1) ^ 1)
        }
    }

    fn vertical_resolution(&self) -> u32 {
        240 << (self.intersects(Self::VERTICAL_RESOLUTION)
            && self.intersects(Self::VERTICAL_INTERLACE)) as u32
    }

    fn is_24bit_color_depth(&self) -> bool {
        self.intersects(Self::DISPLAY_AREA_COLOR_DEPTH)
    }

    fn is_ntsc_video_mode(&self) -> bool {
        !self.intersects(Self::VIDEO_MODE)
    }

    fn _display_enabled(&self) -> bool {
        !self.intersects(Self::DISPLAY_DISABLED)
    }

    fn semi_transparency_mode(&self) -> u8 {
        ((self.bits() & Self::SEMI_TRASPARENCY.bits()) >> 5) as u8
    }

    fn dither_enabled(&self) -> bool {
        self.intersects(Self::DITHER_ENABLED)
    }

    /// Drawing commands that use textures will update gpustat
    fn update_from_texture_params(&mut self, texture_params: &DrawingTextureParams) {
        let x = (texture_params.tex_page_base[0] / 64) & 0xF;
        let y = (texture_params.tex_page_base[1] / 256) & 1;
        *self &= Self::from_bits_retain(!0x81FF);
        *self |= Self::from_bits_retain(x);
        *self |= Self::from_bits_retain(y << 4);
        *self |= Self::from_bits_retain((texture_params.semi_transparency_mode as u32) << 5);
        *self |= Self::from_bits_retain((texture_params.tex_page_color_mode as u32) << 7);
        *self |= Self::from_bits_retain((texture_params.texture_disable as u32) << 15);
    }
}

pub(crate) struct AtomicGpuStat {
    stat: AtomicU32,
}

impl AtomicGpuStat {
    fn new(stat: GpuStat) -> Self {
        Self {
            stat: AtomicU32::new(stat.bits()),
        }
    }

    fn load(&self) -> GpuStat {
        GpuStat::from_bits(self.stat.load(Ordering::Relaxed)).unwrap()
    }

    fn store(&self, stat: GpuStat) {
        self.stat.store(stat.bits(), Ordering::Relaxed);
    }

    fn fetch_update<F>(&self, mut f: F) -> Result<GpuStat, GpuStat>
    where
        F: FnMut(GpuStat) -> Option<GpuStat>,
    {
        self.stat
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                Some(f(GpuStat::from_bits(old).unwrap())?.bits())
            })
            .map(|old| GpuStat::from_bits(old).unwrap())
            .map_err(|e| GpuStat::from_bits(e).unwrap())
    }
}

impl fmt::Debug for AtomicGpuStat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.load())
    }
}

/// The state of the gpu at the execution of the command in the rendering thread
/// Because the state can chanage after setting the command but before execution,
/// we need to send the current state and keep it unmodified until the command is executed.
#[derive(Clone, Default)]
pub(crate) struct GpuStateSnapshot {
    gpu_stat: GpuStat,

    allow_texture_disable: bool,
    textured_rect_flip: (bool, bool),

    drawing_area_top_left: (u32, u32),
    drawing_area_bottom_right: (u32, u32),
    drawing_offset: (i32, i32),
    texture_window_mask: (u32, u32),
    texture_window_offset: (u32, u32),

    vram_display_area_start: (u32, u32),
    display_horizontal_range: (u32, u32),
    display_vertical_range: (u32, u32),

    // These are only used for handleing GP1(0x10) command, so instead of creating
    // the values again from the individual parts, we just cache it
    cached_gp0_e2: u32,
    cached_gp0_e3: u32,
    cached_gp0_e4: u32,
    cached_gp0_e5: u32,
}

#[cfg_attr(not(feature = "vulkan"), allow(dead_code))]
pub(crate) enum BackendCommand {
    BlitFront {
        full_vram: bool,
        state_snapshot: GpuStateSnapshot,
    },
    DrawPolyline {
        vertices: Vec<DrawingVertex>,
        semi_transparent: bool,
        state_snapshot: GpuStateSnapshot,
    },
    DrawPolygon {
        vertices: Vec<DrawingVertex>,
        texture_params: DrawingTextureParams,
        textured: bool,
        texture_blending: bool,
        semi_transparent: bool,
        state_snapshot: GpuStateSnapshot,
    },
    WriteVramBlock {
        block_range: (Range<u32>, Range<u32>),
        block: Vec<u16>,
    },
    VramVramBlit {
        src: (Range<u32>, Range<u32>),
        dst: (Range<u32>, Range<u32>),
    },
    VramReadBlock {
        block_range: (Range<u32>, Range<u32>),
    },
    FillColor {
        top_left: (u32, u32),
        size: (u32, u32),
        color: (u8, u8, u8),
    },
}

#[cfg_attr(not(feature = "vulkan"), allow(dead_code))]
pub(crate) struct Gpu {
    // used for blitting to frontend
    queue: Arc<Queue>,
    device: Arc<Device>,

    // handle the backend gpu thread
    #[cfg(feature = "vulkan")]
    _gpu_backend_thread_handle: JoinHandle<()>,

    /// holds commands that needs extra parameter and complex, like sending
    /// to/from VRAM, and rendering
    current_command: Option<Box<dyn Gp0Command>>,
    // GPUREAD channel
    gpu_read_sender: mpsc::Sender<u32>,
    gpu_read_receiver: PeekableReceiver<u32>,
    // backend commands channel
    gpu_backend_sender: mpsc::Sender<BackendCommand>,
    // channel for front image coming from backend
    gpu_front_image_receiver: mpsc::Receiver<Arc<Image>>,

    first_frame: bool,
    current_front_image: Option<Arc<Image>>,
    command_buffer_allocator: Arc<StandardCommandBufferAllocator>,

    // shared GPUSTAT
    gpu_stat: Arc<AtomicGpuStat>,
    state_snapshot: GpuStateSnapshot,

    scanline: u32,
    dot: u32,
    drawing_odd: bool,
    in_vblank: bool,

    cpu_cycles_counter: u32,
}

impl Gpu {
    pub fn new(device: Arc<Device>, queue: Arc<Queue>) -> Self {
        let (gpu_read_sender, gpu_read_receiver) = mpsc::channel();
        #[allow(unused_variables)]
        let (gpu_backend_sender, gpu_backend_receiver) = mpsc::channel();
        #[allow(unused_variables)]
        let (gpu_front_image_sender, gpu_front_image_receiver) = mpsc::channel();

        let gpu_stat = Arc::new(AtomicGpuStat::new(
            GpuStat::READY_FOR_CMD_RECV | GpuStat::READY_FOR_DMA_RECV,
        ));

        let state_snapshot = GpuStateSnapshot {
            gpu_stat: gpu_stat.load(),
            allow_texture_disable: false,
            textured_rect_flip: (false, false),

            drawing_area_top_left: (0, 0),
            drawing_area_bottom_right: (0, 0),
            drawing_offset: (0, 0),
            texture_window_mask: (0, 0),
            texture_window_offset: (0, 0),

            cached_gp0_e2: 0,
            cached_gp0_e3: 0,
            cached_gp0_e4: 0,
            cached_gp0_e5: 0,

            vram_display_area_start: (0, 0),
            display_horizontal_range: (0, 0),
            display_vertical_range: (0, 0),
        };

        #[cfg(feature = "vulkan")]
        let _gpu_backend_thread_handle = backend::GpuBackend::start(
            device.clone(),
            queue.clone(),
            gpu_stat.clone(),
            gpu_read_sender.clone(),
            gpu_backend_receiver,
            gpu_front_image_sender,
        );

        Self {
            queue,
            device: device.clone(),

            #[cfg(feature = "vulkan")]
            _gpu_backend_thread_handle,

            current_command: None,
            gpu_read_sender,
            gpu_read_receiver: PeekableReceiver::new(gpu_read_receiver),
            gpu_backend_sender,
            gpu_front_image_receiver,

            first_frame: true,
            current_front_image: None,
            command_buffer_allocator: Arc::new(StandardCommandBufferAllocator::new(
                device,
                Default::default(),
            )),

            gpu_stat,
            state_snapshot,

            scanline: 0,
            dot: 0,
            drawing_odd: false,
            in_vblank: false,
            cpu_cycles_counter: 0,
        }
    }

    pub fn reset(&mut self) {
        let _ = std::mem::replace(self, Self::new(self.device.clone(), self.queue.clone()));
    }

    /// returns the number of `dot_clocks`, and if `hblank_clock` occurres
    /// when clocking the gpu for `cycles` cycles.
    /// These clocks are used for timers.
    pub fn clock(
        &mut self,
        interrupt_requester: &mut impl InterruptRequester,
        cpu_cycles: u32,
    ) -> (u32, bool) {
        // The GPU clock is CPU*11/7 == 53.222400MHz
        // The FPS is determined by the mode, NTSC is 60~Hz, PAL is 50~Hz
        self.cpu_cycles_counter += cpu_cycles * 11;

        let cycles = self.cpu_cycles_counter / 7;
        self.cpu_cycles_counter %= 7;

        let gpu_stat = self.gpu_stat.load();
        let max_dots = if gpu_stat.is_ntsc_video_mode() {
            3413
        } else {
            3406
        };
        let max_scanlines = if gpu_stat.is_ntsc_video_mode() {
            263
        } else {
            314
        };
        let horizontal_dots_divider = gpu_stat.horizontal_dots_divider();
        let vertical_resolution = gpu_stat.vertical_resolution();
        let is_interlace = gpu_stat.intersects(GpuStat::VERTICAL_INTERLACE);

        // we can't overflow the max_dots and clock for example more than one
        // scanline at a time.
        assert!(cycles < max_dots);
        self.dot += cycles;

        // If the increment is more than the divider, we will clock the timer by the number
        // of times the divider fits in the increment.
        let mut dot_clocks = cycles / horizontal_dots_divider;

        // We may have extra cycles to clock for one more time.
        // For example:
        // - divider = 10
        // - cycles = 15
        // If we follow the cycles increment, we will skip one value:
        // 0 -> 15 -> 30. We lose the increment, when we got to `20` we will lose the
        // `dot_clock`, but with the following check, we can know that we missed it
        // and handle it accordingly.
        if (self.dot % horizontal_dots_divider) < (cycles % horizontal_dots_divider) {
            dot_clocks += 1;
        }

        let mut hblank_clock = false;
        if self.dot >= max_dots {
            hblank_clock = true;
            self.dot -= max_dots;
            self.scanline += 1;

            if is_interlace && vertical_resolution == 240 && self.scanline < 240 {
                self.drawing_odd = !self.drawing_odd;
            }

            if self.scanline >= max_scanlines {
                self.scanline = 0;
                self.in_vblank = false;

                if is_interlace && vertical_resolution == 480 {
                    self.drawing_odd = !self.drawing_odd;
                }
            }

            if self.scanline == 240 {
                interrupt_requester.request_vblank();
                self.in_vblank = true;
            }
        }

        (dot_clocks, hblank_clock)
    }

    pub fn in_vblank(&self) -> bool {
        self.in_vblank
    }

    #[cfg(not(feature = "vulkan"))]
    pub fn sync_gpu_and_blit_to_front(
        &mut self,
        _dest_image: Arc<Image>,
        _full_vram: bool,
        in_future: Box<dyn GpuFuture>,
    ) -> Box<dyn GpuFuture> {
        in_future
    }

    #[cfg(feature = "vulkan")]
    pub fn sync_gpu_and_blit_to_front(
        &mut self,
        dest_image: Arc<Image>,
        full_vram: bool,
        in_future: Box<dyn GpuFuture>,
    ) -> Box<dyn GpuFuture> {
        // if we have a previous image, then we are not in the first frame,
        // so there should be an image in the channel.
        if !self.first_frame {
            // `recv` is blocking, here we will wait for the GPU to finish all drawing.
            // FIXME: Do not block. Find a way to keep the GPU synced with minimal performance loss.
            self.current_front_image = Some(self.gpu_front_image_receiver.recv().unwrap());
        }
        self.first_frame = false;

        // send command for next frame from now, so when we recv later, its mostly will be ready
        self.state_snapshot.gpu_stat = self.gpu_stat.load();
        self.gpu_backend_sender
            .send(BackendCommand::BlitFront {
                full_vram,
                state_snapshot: self.state_snapshot.clone(),
            })
            .unwrap();

        if let Some(img) = self.current_front_image.as_ref() {
            let mut builder: AutoCommandBufferBuilder<
                crate::gpu::vulkan::PrimaryAutoCommandBuffer,
            > = AutoCommandBufferBuilder::primary(
                self.command_buffer_allocator.clone(),
                self.queue.queue_family_index(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

            builder
                .blit_image(BlitImageInfo {
                    filter: Filter::Nearest,
                    ..BlitImageInfo::images(img.clone(), dest_image)
                })
                .unwrap();
            let cb = builder.build().unwrap();

            // TODO: remove wait
            in_future
                .then_execute(self.queue.clone(), cb)
                .unwrap()
                .then_signal_fence_and_flush()
                .unwrap()
                .boxed()
        } else {
            // we must flush the future even if we are not using it.
            in_future
        }
    }
}

impl Gpu {
    fn read_gpu_stat(&self) -> u32 {
        let interlace_bit = (self.drawing_odd && !self.in_vblank) as u32;
        // set by GP1(0x8)
        let interlace_field = if self.gpu_stat.load().intersects(GpuStat::INTERLACE_FIELD) {
            1 // always on
        } else {
            interlace_bit ^ 1
        };

        // Ready to receive Cmd Word
        // Ready to receive DMA Block
        let out = self.gpu_stat.load().bits() | (interlace_bit << 31) | (interlace_field << 13);
        log::trace!("GPUSTAT = {:08X}", out);
        log::trace!("GPUSTAT = {:?}", self.gpu_stat);
        out
    }

    fn gpu_read(&mut self) -> u32 {
        let out = self.gpu_read_receiver.try_recv();

        if self.gpu_read_receiver.is_empty() {
            self.gpu_stat
                .fetch_update(|s| Some(s - GpuStat::READY_FOR_TO_SEND_VRAM))
                .unwrap();
        }

        log::trace!("GPUREAD = {:08X?}", out);
        out.unwrap_or(0)
    }
}
impl Gpu {
    /// handles creating Gp0 commands, and then when ready to be executed,
    /// will be sent to the backend.
    fn handle_gp0(&mut self, data: u32) {
        log::trace!("GPU: GP0 write: {:08x}", data);
        // if we still executing some command
        if let Some(cmd) = self.current_command.as_mut() {
            if cmd.still_need_params() {
                log::trace!("gp0 extra param {:08X}", data);
                cmd.add_param(data);
                if !cmd.still_need_params() {
                    let cmd = self.current_command.take().unwrap();

                    self.gpu_stat
                        .fetch_update(|s| Some(s - GpuStat::READY_FOR_DMA_RECV))
                        .unwrap();

                    log::info!("executing command {:?}", cmd.cmd_type());
                    if let Some(backend_cmd) =
                        cmd.exec_command(self.gpu_stat.clone(), &mut self.state_snapshot)
                    {
                        self.gpu_backend_sender.send(backend_cmd).unwrap();
                    }

                    // ready for next command
                    self.gpu_stat
                        .fetch_update(|s| {
                            Some(s | GpuStat::READY_FOR_CMD_RECV | GpuStat::READY_FOR_DMA_RECV)
                        })
                        .unwrap();
                }
            } else {
                unreachable!();
            }
        } else {
            let mut cmd = instantiate_gp0_command(data);
            log::info!("creating new command {:?}", cmd.cmd_type());
            if cmd.still_need_params() {
                self.current_command = Some(cmd);
                self.gpu_stat
                    .fetch_update(|s| Some(s - GpuStat::READY_FOR_CMD_RECV))
                    .unwrap();
            } else {
                log::info!("executing command {:?}", cmd.cmd_type());
                if let Some(backend_cmd) =
                    cmd.exec_command(self.gpu_stat.clone(), &mut self.state_snapshot)
                {
                    self.gpu_backend_sender.send(backend_cmd).unwrap();
                }
            }
        }
    }

    /// Execute instructions we can from frontend, or else send to backend.
    /// This allows for GPU_STAT register to be synced.
    fn handle_gp1(&mut self, data: u32) {
        let cmd = data >> 24;
        log::trace!("gp1 command {:02X} data: {:08X}", cmd, data);
        match cmd {
            0x00 => {
                // Reset Gpu
                // TODO: check what we need to do in reset
                self.gpu_stat.store(
                    GpuStat::DISPLAY_DISABLED
                        | GpuStat::INTERLACE_FIELD
                        | GpuStat::READY_FOR_DMA_RECV
                        | GpuStat::READY_FOR_CMD_RECV,
                );
            }
            0x01 => {
                // Reset command fifo buffer

                if let Some(cmd) = &mut self.current_command {
                    if let Gp0CmdType::CpuToVramBlit = cmd.cmd_type() {
                        // flush vram write

                        let cmd = self.current_command.take().unwrap();
                        // CpuToVramBlit supports interrupts, and will only send
                        // the rows that are written to the vram.
                        if let Some(backend_cmd) =
                            cmd.exec_command(self.gpu_stat.clone(), &mut self.state_snapshot)
                        {
                            self.gpu_backend_sender.send(backend_cmd).unwrap();
                        }
                    }
                }
                self.current_command = None;
            }
            0x02 => {
                // Reset IRQ
                self.gpu_stat
                    .fetch_update(|s| Some(s.difference(GpuStat::INTERRUPT_REQUEST)))
                    .unwrap();
            }
            0x03 => {
                // Display enable
                self.gpu_stat
                    .fetch_update(|s| {
                        if data & 1 == 1 {
                            Some(s.union(GpuStat::DISPLAY_DISABLED))
                        } else {
                            Some(s.difference(GpuStat::DISPLAY_DISABLED))
                        }
                    })
                    .unwrap();
            }
            0x04 => {
                // DMA direction
                // TODO: should also affect GpuStat::DMA_DATA_REQUEST
                self.gpu_stat
                    .fetch_update(|mut s| {
                        s.remove(GpuStat::DMA_DIRECTION);
                        s |= GpuStat::from_bits_retain((data & 3) << 29);
                        Some(s)
                    })
                    .unwrap();
            }
            0x05 => {
                // Vram Start of Display area

                let x = data & 0x3ff;
                let y = (data >> 10) & 0x1ff;

                self.state_snapshot.vram_display_area_start = (x, y);
                log::info!(
                    "vram display start area {:?}",
                    self.state_snapshot.vram_display_area_start
                );
            }
            0x06 => {
                // Screen Horizontal Display range
                let x1 = data & 0xfff;
                let x2 = (data >> 12) & 0xfff;

                self.state_snapshot.display_horizontal_range = (x1, x2);
                log::info!(
                    "display horizontal range {:?}",
                    self.state_snapshot.display_horizontal_range
                );
            }
            0x07 => {
                // Screen Vertical Display range
                let y1 = data & 0x1ff;
                let y2 = (data >> 10) & 0x1ff;

                self.state_snapshot.display_vertical_range = (y1, y2);
                log::info!(
                    "display vertical range {:?}",
                    self.state_snapshot.display_vertical_range
                );
            }
            0x08 => {
                // Display mode

                // 17-18 Horizontal Resolution 1     (0=256, 1=320, 2=512, 3=640)
                // 19    Vertical Resolution         (0=240, 1=480, when Bit22=1)
                // 20    Video Mode                  (0=NTSC/60Hz, 1=PAL/50Hz)
                // 21    Display Area Color Depth    (0=15bit, 1=24bit)
                // 22    Vertical Interlace          (0=Off, 1=On)
                let stat_bits_17_22 = data & 0x3F;
                let stat_bit_16_horizontal_resolution_2 = (data >> 6) & 1;
                let stat_bit_14_reverse_flag = (data >> 7) & 1;
                // the inverse of the vertical interlace
                let interlace_field = ((data >> 5) & 1) ^ 1;

                self.gpu_stat
                    .fetch_update(|mut s| {
                        s &= GpuStat::from_bits_retain(!0x7f6000);
                        s |= GpuStat::from_bits_retain(stat_bits_17_22 << 17);
                        s |= GpuStat::from_bits_retain(stat_bit_14_reverse_flag << 14);
                        s |= GpuStat::from_bits_retain(stat_bit_16_horizontal_resolution_2 << 16);
                        s |= GpuStat::from_bits_retain(interlace_field << 13);
                        Some(s)
                    })
                    .unwrap();
            }
            0x09 => {
                // Allow texture disable
                self.state_snapshot.allow_texture_disable = data & 1 == 1;
            }
            0x10 => {
                // GPU info

                // 0x0~0xF retreive info, and the rest are mirrors
                let info_id = data & 0xF;

                let result = match info_id {
                    2 => {
                        // Read Texture Window setting GP0(E2h)
                        self.state_snapshot.cached_gp0_e2
                    }
                    3 => {
                        // Read Draw area top left GP0(E3h)
                        self.state_snapshot.cached_gp0_e3
                    }
                    4 => {
                        // Read Draw area bottom right GP0(E4h)
                        self.state_snapshot.cached_gp0_e4
                    }
                    5 => {
                        // Read Draw offset GP0(E5h)
                        self.state_snapshot.cached_gp0_e5
                    }
                    6 => {
                        // TODO: return old value of GPUREAD
                        0
                    }
                    7 => {
                        // GPU type
                        2
                    }
                    8 => {
                        // unknown
                        0
                    }
                    _ => {
                        // TODO: return old value of GPUREAD
                        0
                    }
                };

                self.gpu_read_sender.send(result).unwrap();
            }
            _ => todo!("gp1 command {:02X}", cmd),
        }
    }
}

impl BusLine for Gpu {
    fn read_u32(&mut self, addr: u32) -> Result<u32> {
        let r = match addr {
            0 => self.gpu_read(),
            4 => self.read_gpu_stat(),
            _ => unreachable!(),
        };
        Ok(r)
    }

    fn write_u32(&mut self, addr: u32, data: u32) -> Result<()> {
        match addr {
            0 => {
                self.handle_gp0(data);
            }
            4 => {
                self.handle_gp1(data);
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}
