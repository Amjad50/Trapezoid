mod command;
mod front_blit;
mod gpu_backend;
mod gpu_context;

use crate::memory::{interrupts::InterruptRequester, BusLine};
use command::{instantiate_gp0_command, Gp0CmdType, Gp0Command};
use gpu_backend::GpuBackend;
use gpu_context::GpuContext;

use crossbeam::{
    atomic::AtomicCell,
    channel::{Receiver, Sender},
};
use vulkano::{
    command_buffer::{
        allocator::StandardCommandBufferAllocator, AutoCommandBufferBuilder, BlitImageInfo,
        CommandBufferUsage, PrimaryAutoCommandBuffer,
    },
    device::{Device, Queue},
    image::{ImageAccess, StorageImage},
    sampler::Filter,
    sync::GpuFuture,
};

use std::{
    sync::{Arc, Mutex},
    thread::JoinHandle,
};

use self::gpu_context::GpuSharedState;

bitflags::bitflags! {
    #[derive(Default)]
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

impl GpuStat {
    fn _texture_page_coords(&self) -> (u32, u32) {
        let x = (self.bits & Self::TEXTURE_PAGE_X_BASE.bits) * 64;
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
            let resolution_multiplier = (self.bits & Self::HORIZONTAL_RESOLUTION1.bits) >> 17;
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
            // 0: 8
            // 1: 10
            // 2: 4
            // 3: 5
            //
            // The second two numbers are half the first two, so we can use the
            // second bit to divide by 2.
            let resolution_bits = (self.bits & Self::HORIZONTAL_RESOLUTION1.bits) >> 17;

            // add 2 if the first bit is set
            let base = 8 + ((resolution_bits & 1) * 2);
            // divide by 2 if the second bit is set
            base >> (resolution_bits >> 1)
        }
    }

    fn vertical_resolution(&self) -> u32 {
        240 << self.intersects(Self::VERTICAL_RESOLUTION) as u32
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
        ((self.bits & Self::SEMI_TRASPARENCY.bits) >> 5) as u8
    }

    fn dither_enabled(&self) -> bool {
        self.intersects(Self::DITHER_ENABLED)
    }
}

enum BackendCommand {
    BlitFront(bool),
    GpuCommand(Box<dyn Gp0Command>),
}

pub struct Gpu {
    // used for blitting to frontend
    queue: Arc<Queue>,

    // handle the backend gpu thread
    _gpu_backend_thread_handle: JoinHandle<()>,

    /// holds commands that needs extra parameter and complex, like sending
    /// to/from VRAM, and rendering
    current_command: Option<Box<dyn Gp0Command>>,
    // GPUREAD channel
    gpu_read_sender: Sender<u32>,
    gpu_read_receiver: Receiver<u32>,
    // backend commands channel
    gpu_backend_sender: Sender<BackendCommand>,
    // channel for front image coming from backend
    gpu_front_image_receiver: Receiver<Arc<StorageImage>>,

    first_frame: bool,
    current_front_image: Option<Arc<StorageImage>>,
    command_buffer_allocator: StandardCommandBufferAllocator,

    // shared GPUSTAT
    gpu_stat: Arc<AtomicCell<GpuStat>>,
    shared_state: Arc<Mutex<GpuSharedState>>,

    scanline: u32,
    dot: u32,
    drawing_odd: bool,
    in_vblank: bool,

    cpu_cycles_counter: u32,
}

impl Gpu {
    pub fn new(device: Arc<Device>, queue: Arc<Queue>) -> Self {
        let (gpu_read_sender, gpu_read_receiver) = crossbeam::channel::unbounded();
        let (gpu_backend_sender, gpu_backend_receiver) = crossbeam::channel::unbounded();
        let (gpu_front_image_sender, gpu_front_image_receiver) = crossbeam::channel::unbounded();

        let gpu_stat = Arc::new(AtomicCell::new(
            GpuStat::READY_FOR_CMD_RECV | GpuStat::READY_FOR_DMA_RECV,
        ));

        let shared_state = Arc::new(Mutex::new(GpuSharedState {
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
        }));

        let _gpu_backend_thread_handle = GpuBackend::start(
            device.clone(),
            queue.clone(),
            gpu_stat.clone(),
            shared_state.clone(),
            gpu_read_sender.clone(),
            gpu_backend_receiver,
            gpu_front_image_sender,
        );

        Self {
            queue,

            _gpu_backend_thread_handle,

            current_command: None,
            gpu_read_sender,
            gpu_read_receiver,
            gpu_backend_sender,
            gpu_front_image_receiver,

            first_frame: true,
            current_front_image: None,
            command_buffer_allocator: StandardCommandBufferAllocator::new(
                device,
                Default::default(),
            ),

            gpu_stat,
            shared_state,

            scanline: 0,
            dot: 0,
            drawing_odd: false,
            in_vblank: false,
            cpu_cycles_counter: 0,
        }
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

    pub fn sync_gpu_and_blit_to_front<D>(
        &mut self,
        dest_image: Arc<D>,
        full_vram: bool,
        in_future: Box<dyn GpuFuture>,
    ) -> Box<dyn GpuFuture>
    where
        D: ImageAccess + 'static,
    {
        // if we have a previous image, then we are not in the first frame,
        // so there should be an image in the channel.
        if !self.first_frame {
            // `recv` is blocking, here we will wait for the GPU to finish all drawing.
            // FIXME: Do not block. Find a way to keep the GPU synced with minimal performance loss.
            self.current_front_image = Some(self.gpu_front_image_receiver.recv().unwrap());
        }
        self.first_frame = false;

        // send command for next frame from now, so when we recv later, its mostly will be ready
        self.gpu_backend_sender
            .send(BackendCommand::BlitFront(full_vram))
            .unwrap();

        if let Some(img) = self.current_front_image.as_ref() {
            let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
                AutoCommandBufferBuilder::primary(
                    &self.command_buffer_allocator,
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
        // Ready to receive Cmd Word
        // Ready to receive DMA Block
        let out =
            self.gpu_stat.load().bits | (((self.drawing_odd && !self.in_vblank) as u32) << 31);

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
                    // take the self reference from here, so that we can update the gpu_stat
                    // without issues
                    let cmd = self.current_command.take().unwrap();

                    self.gpu_stat
                        .fetch_update(|s| Some(s - GpuStat::READY_FOR_DMA_RECV))
                        .unwrap();

                    log::info!("executing command {:?}", cmd.cmd_type());
                    self.gpu_backend_sender
                        .send(BackendCommand::GpuCommand(cmd))
                        .unwrap();

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
            // some env commands must be synced
            if data >> 29 == 7 {
                let mut shared_state = self.shared_state.lock().unwrap();
                let cmd = data >> 24;
                match cmd {
                    0xe1 => {
                        // NOTE: this is also duplicated in the frontend for keeping stat up to date
                        // Draw Mode setting

                        // 0-3   Texture page X Base   (N*64)
                        // 4     Texture page Y Base   (N*256)
                        // 5-6   Semi Transparency     (0=B/2+F/2, 1=B+F, 2=B-F, 3=B+F/4)
                        // 7-8   Texture page colors   (0=4bit, 1=8bit, 2=15bit, 3=Reserved)
                        // 9     Dither 24bit to 15bit (0=Off/strip LSBs, 1=Dither Enabled)
                        // 10    Drawing to display area (0=Prohibited, 1=Allowed)
                        // 11    Texture Disable (0=Normal, 1=Disable if GP1(09h).Bit0=1)   ;GPUSTAT.15
                        let stat_lower_11_bits = data & 0x7FF;
                        let stat_bit_15_texture_disable = (data >> 11) & 1 == 1;

                        let textured_rect_x_flip = (data >> 12) & 1 == 1;
                        let textured_rect_y_flip = (data >> 13) & 1 == 1;
                        shared_state.textured_rect_flip =
                            (textured_rect_x_flip, textured_rect_y_flip);

                        self.gpu_stat
                            .fetch_update(|mut s| {
                                s.bits &= !0x87FF;
                                s.bits |= stat_lower_11_bits;
                                if stat_bit_15_texture_disable && shared_state.allow_texture_disable
                                {
                                    s.bits |= 1 << 15;
                                }
                                Some(s)
                            })
                            .unwrap();
                    }
                    0xe2 => {
                        shared_state.cached_gp0_e2 = data;

                        // Texture window settings
                        let mask_x = data & 0x1F;
                        let mask_y = (data >> 5) & 0x1F;
                        let offset_x = (data >> 10) & 0x1F;
                        let offset_y = (data >> 15) & 0x1F;

                        shared_state.texture_window_mask = (mask_x, mask_y);
                        shared_state.texture_window_offset = (offset_x, offset_y);

                        log::info!(
                            "texture window mask = {:?}, offset = {:?}",
                            shared_state.texture_window_mask,
                            shared_state.texture_window_offset
                        );
                    }
                    0xe3 => {
                        shared_state.cached_gp0_e3 = data;

                        // Set Drawing Area top left
                        let x = data & 0x3ff;
                        let y = (data >> 10) & 0x3ff;
                        shared_state.drawing_area_top_left = (x, y);
                        log::info!(
                            "drawing area top left = {:?}",
                            shared_state.drawing_area_top_left,
                        );
                    }
                    0xe4 => {
                        shared_state.cached_gp0_e4 = data;

                        // Set Drawing Area bottom right
                        let x = data & 0x3ff;
                        let y = (data >> 10) & 0x3ff;
                        shared_state.drawing_area_bottom_right = (x, y);
                        log::info!(
                            "drawing area bottom right = {:?}",
                            shared_state.drawing_area_bottom_right,
                        );
                    }
                    0xe5 => {
                        shared_state.cached_gp0_e5 = data;

                        // Set Drawing offset
                        // TODO: test the accuracy of the sign extension
                        let x = data & 0x7ff;
                        let sign_extend = 0xfffff800 * ((x >> 10) & 1);
                        let x = (x | sign_extend) as i32;
                        let y = (data >> 11) & 0x7ff;
                        let sign_extend = 0xfffff800 * ((y >> 10) & 1);
                        let y = (y | sign_extend) as i32;
                        shared_state.drawing_offset = (x, y);
                        log::info!("drawing offset = {:?}", shared_state.drawing_offset,);
                    }
                    0xe6 => {
                        // NOTE: this is also duplicated in the frontend for keeping stat up to date
                        // Mask Bit Setting

                        //  11    Set mask while drawing (0=TextureBit15, 1=ForceBit15=1)
                        //  12    Check mask before draw (0=Draw Always, 1=Draw if Bit15=0)
                        let stat_bits_11_12 = data & 3;

                        self.gpu_stat
                            .fetch_update(|mut s| {
                                s.bits &= !(3 << 11);
                                s.bits |= stat_bits_11_12;
                                Some(s)
                            })
                            .unwrap();
                    }
                    _ => todo!("gp0 environment command {:02X}", cmd),
                }
                return;
            }

            let mut cmd = instantiate_gp0_command(data);
            log::info!("creating new command {:?}", cmd.cmd_type());
            if cmd.still_need_params() {
                self.current_command = Some(cmd);
                self.gpu_stat
                    .fetch_update(|s| Some(s - GpuStat::READY_FOR_CMD_RECV))
                    .unwrap();
            } else {
                log::info!("executing command {:?}", cmd.cmd_type());
                self.gpu_backend_sender
                    .send(BackendCommand::GpuCommand(cmd))
                    .unwrap();
            }
        }
    }

    /// Execute instructions we can from frontend, or else send to backend.
    /// This allows for GPU_STAT register to be synced.
    fn handle_gp1(&mut self, data: u32) {
        let cmd = data >> 24;
        log::trace!("gp1 command {:02X} data: {:08X}", cmd, data);
        let mut shared_state = self.shared_state.lock().unwrap();
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

                // TODO: reset the sender buffer
                if let Some(cmd) = &mut self.current_command {
                    if let Gp0CmdType::CpuToVramBlit = cmd.cmd_type() {
                        // flush vram write

                        // FIXME: close the write here and flush
                        //  do not add more data
                        //while !cmd.exec_command(&mut self.gpu_context) {
                        //    if cmd.still_need_params() {
                        //        cmd.add_param(0);
                        //    }
                        //}
                        self.current_command = None;
                        todo!();
                    }
                }
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
                        s.bits |= (data & 3) << 29;
                        Some(s)
                    })
                    .unwrap();
            }
            0x05 => {
                // Vram Start of Display area

                let x = data & 0x3ff;
                let y = (data >> 10) & 0x1ff;

                shared_state.vram_display_area_start = (x, y);
                log::info!(
                    "vram display start area {:?}",
                    shared_state.vram_display_area_start
                );
            }
            0x06 => {
                // Screen Horizontal Display range
                let x1 = data & 0xfff;
                let x2 = (data >> 12) & 0xfff;

                shared_state.display_horizontal_range = (x1, x2);
                log::info!(
                    "display horizontal range {:?}",
                    shared_state.display_horizontal_range
                );
            }
            0x07 => {
                // Screen Vertical Display range
                let y1 = data & 0x1ff;
                let y2 = (data >> 10) & 0x1ff;

                shared_state.display_vertical_range = (y1, y2);
                log::info!(
                    "display vertical range {:?}",
                    shared_state.display_vertical_range
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
                        s.bits &= !0x7f6000;
                        s.bits |= stat_bits_17_22 << 17;
                        s.bits |= stat_bit_14_reverse_flag << 14;
                        s.bits |= stat_bit_16_horizontal_resolution_2 << 16;
                        s.bits |= interlace_field << 13;
                        Some(s)
                    })
                    .unwrap();
            }
            0x09 => {
                // Allow texture disable
                shared_state.allow_texture_disable = data & 1 == 1;
            }
            0x10 => {
                // GPU info

                // 0x0~0xF retreive info, and the rest are mirrors
                let info_id = data & 0xF;

                // TODO: we don't need to be empty in our design, but we
                //       need the old data in some commands, so for now,
                //       lets make sure we don't have old data, until we
                //       store it somewhere.
                assert!(self.gpu_read_sender.is_empty());

                // TODO: some commands read old value of GPUREAD, we can't do that
                // now. might need to change how we handle GPUREAD in general
                let result = match info_id {
                    2 => {
                        // Read Texture Window setting GP0(E2h)
                        shared_state.cached_gp0_e2
                    }
                    3 => {
                        // Read Draw area top left GP0(E3h)
                        shared_state.cached_gp0_e3
                    }
                    4 => {
                        // Read Draw area bottom right GP0(E4h)
                        shared_state.cached_gp0_e4
                    }
                    5 => {
                        // Read Draw offset GP0(E5h)
                        shared_state.cached_gp0_e5
                    }
                    6 => {
                        // return old value of GPUREAD
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
                        // return old value of GPUREAD
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
    fn read_u32(&mut self, addr: u32) -> u32 {
        match addr {
            0 => self.gpu_read(),
            4 => self.read_gpu_stat(),
            _ => unreachable!(),
        }
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        match addr {
            0 => {
                self.handle_gp0(data);
            }
            4 => {
                self.handle_gp1(data);
            }
            _ => unreachable!(),
        }
    }

    fn read_u16(&mut self, _addr: u32) -> u16 {
        todo!()
    }

    fn write_u16(&mut self, _addr: u32, _data: u16) {
        todo!()
    }

    fn read_u8(&mut self, _addr: u32) -> u8 {
        todo!()
    }

    fn write_u8(&mut self, _addr: u32, _data: u8) {
        todo!()
    }
}
