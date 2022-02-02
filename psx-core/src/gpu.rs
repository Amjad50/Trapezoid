mod command;
mod front_blit;
mod gpu_backend;
mod gpu_context;

use crate::memory::{interrupts::InterruptRequester, BusLine};
use std::{sync::Arc, thread::JoinHandle};

use crossbeam::{
    atomic::AtomicCell,
    channel::{Receiver, Sender},
};
use gpu_backend::GpuBackend;
use gpu_context::GpuContext;
use vulkano::{
    command_buffer::{AutoCommandBufferBuilder, CommandBufferUsage, PrimaryAutoCommandBuffer},
    device::{Device, Queue},
    image::{ImageAccess, StorageImage},
    sampler::Filter,
    sync::GpuFuture,
};

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
    Gp0Write(u32),
    Gp1Write(u32),
    BlitFront(bool),
}

pub struct Gpu {
    // used for blitting to frontend
    device: Arc<Device>,
    queue: Arc<Queue>,

    // handle the backend gpu thread
    _gpu_backend_thread_handle: JoinHandle<()>,

    // GPUREAD channel
    gpu_read_receiver: Receiver<u32>,
    // backend commands channel
    gpu_backend_sender: Sender<BackendCommand>,
    // channel for front image coming from backend
    gpu_front_image_receiver: Receiver<Arc<StorageImage>>,

    current_front_image: Option<Arc<StorageImage>>,

    // shared GPUSTAT
    gpu_stat: Arc<AtomicCell<GpuStat>>,

    scanline: u32,
    dot: u32,
    drawing_odd: bool,
    in_vblank: bool,
}

impl Gpu {
    pub fn new(device: Arc<Device>, queue: Arc<Queue>) -> Self {
        let (gpu_read_sender, gpu_read_receiver) = crossbeam::channel::unbounded();
        let (gpu_backend_sender, gpu_backend_receiver) = crossbeam::channel::unbounded();
        let (gpu_front_image_sender, gpu_front_image_receiver) = crossbeam::channel::unbounded();

        let gpu_stat = Arc::new(AtomicCell::new(
            GpuStat::READY_FOR_CMD_RECV | GpuStat::READY_FOR_DMA_RECV,
        ));

        let _gpu_backend_thread_handle = GpuBackend::start(
            device.clone(),
            queue.clone(),
            gpu_stat.clone(),
            gpu_read_sender,
            gpu_backend_receiver,
            gpu_front_image_sender,
        );

        Self {
            device,
            queue,

            _gpu_backend_thread_handle,
            gpu_read_receiver,
            gpu_backend_sender,
            gpu_front_image_receiver,

            current_front_image: None,

            gpu_stat,

            scanline: 0,
            dot: 0,
            drawing_odd: false,
            in_vblank: false,
        }
    }

    /// returns the number of `dot_clocks`, and if `hblank_clock` occurres
    /// when clocking the gpu for `cycles` cycles.
    /// These clocks are used for timers.
    pub fn clock(
        &mut self,
        interrupt_requester: &mut impl InterruptRequester,
        cycles: u32,
    ) -> (u32, bool) {
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

    pub fn blit_to_front<D, IF>(&mut self, dest_image: Arc<D>, full_vram: bool, in_future: IF)
    where
        D: ImageAccess + 'static,
        IF: GpuFuture,
    {
        self.gpu_backend_sender
            .send(BackendCommand::BlitFront(full_vram))
            .unwrap();

        // may not be ready yet
        let img = self.gpu_front_image_receiver.try_recv().ok();
        self.current_front_image = img.or(self.current_front_image.take());

        if let Some(img) = self.current_front_image.as_ref() {
            let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
                AutoCommandBufferBuilder::primary(
                    self.device.clone(),
                    self.queue.family(),
                    CommandBufferUsage::OneTimeSubmit,
                )
                .unwrap();

            builder
                .blit_image(
                    img.clone(),
                    [0, 0, 0],
                    [
                        img.dimensions().width() as i32,
                        img.dimensions().height() as i32,
                        1,
                    ],
                    0,
                    0,
                    dest_image.clone(),
                    [0, 0, 0],
                    [
                        dest_image.dimensions().width() as i32,
                        dest_image.dimensions().height() as i32,
                        1,
                    ],
                    0,
                    0,
                    1,
                    Filter::Nearest,
                )
                .unwrap();
            let cb = builder.build().unwrap();

            // TODO: remove wait
            in_future
                .then_execute(self.queue.clone(), cb)
                .unwrap()
                .then_signal_fence_and_flush()
                .unwrap()
                .wait(None)
                .unwrap();
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
            0 => self
                .gpu_backend_sender
                .send(BackendCommand::Gp0Write(data))
                .unwrap(),
            4 => self
                .gpu_backend_sender
                .send(BackendCommand::Gp1Write(data))
                .unwrap(),
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
