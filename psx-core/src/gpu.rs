mod command;
mod gpu_context;

use crate::memory::{interrupts::InterruptRequester, BusLine};
use std::collections::VecDeque;

use command::{instantiate_gp0_command, Gp0CmdType, Gp0Command};
pub use gpu_context::GlContext;
use gpu_context::GpuContext;

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
    fn texture_page_coords(&self) -> (u32, u32) {
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
            let resolution_multiplier = (self.bits & Self::HORIZONTAL_RESOLUTION1.bits) >> 17;
            let resoltion = 0x100 | ((resolution_multiplier & 1) << 6);
            resoltion << (resolution_multiplier >> 1)
        }
    }

    fn vertical_resolution(&self) -> u32 {
        240 << self.intersects(Self::VERTICAL_RESOLUTION) as u32
    }

    fn is_ntsc_video_mode(&self) -> bool {
        !self.intersects(Self::VIDEO_MODE)
    }

    fn display_enabled(&self) -> bool {
        !self.intersects(Self::DISPLAY_DISABLED)
    }

    fn semi_transparency_mode(&self) -> u8 {
        ((self.bits & Self::SEMI_TRASPARENCY.bits) >> 5) as u8
    }
}

pub struct Gpu {
    gpu_context: GpuContext,
    /// holds commands that needs extra parameter and complex, like sending
    /// to/from VRAM, and rendering
    current_command: Option<Box<dyn Gp0Command>>,
    // TODO: replace by fixed vec deque to not exceed the limited size
    command_fifo: VecDeque<u32>,

    scanline: u32,
    dot: u32,
    drawing_odd: bool,
    in_vblank: bool,
}

// for easier access to gpu context
impl std::ops::Deref for Gpu {
    type Target = GpuContext;

    fn deref(&self) -> &Self::Target {
        &self.gpu_context
    }
}

// for easier access to gpu context
impl std::ops::DerefMut for Gpu {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.gpu_context
    }
}

impl Gpu {
    pub fn new(gl_context: GlContext) -> Self {
        Self {
            gpu_context: GpuContext::new(gl_context),
            current_command: None,
            command_fifo: VecDeque::new(),
            scanline: 0,
            dot: 0,
            drawing_odd: false,
            in_vblank: false,
        }
    }

    pub fn clock(&mut self, interrupt_requester: &mut impl InterruptRequester) {
        self.drawing_clock(interrupt_requester);
        self.clock_gp0_command();
    }

    pub fn in_vblank(&self) -> bool {
        self.in_vblank
    }

    pub fn blit_to_front<S: glium::Surface>(&self, s: &S, full_vram: bool) {
        self.gpu_context.blit_to_front(s, full_vram);
    }
}

impl Gpu {
    fn gpu_stat(&self) -> u32 {
        // Ready to receive Cmd Word
        // Ready to receive DMA Block
        let out = self.gpu_stat.bits
            | (0b101 << 26)
            | ((self.gpu_read.is_some() as u32) << 27)
            | (((self.drawing_odd && !self.in_vblank) as u32) << 31);

        log::info!("GPUSTAT = {:08X}", out);
        log::info!("GPUSTAT = {:?}", self.gpu_stat);
        out
    }

    fn gpu_read(&mut self) -> u32 {
        let out = self.gpu_read.take().unwrap_or(0);
        // clock the command, so that it works with DMA reads
        self.clock_gp0_command();
        log::info!("GPUREAD = {:08X}", out);
        out
    }

    fn drawing_clock(&mut self, interrupt_requester: &mut impl InterruptRequester) {
        let max_dots = if self.gpu_stat.is_ntsc_video_mode() {
            3413
        } else {
            3406
        };
        let max_scanlines = if self.gpu_stat.is_ntsc_video_mode() {
            263
        } else {
            314
        };
        let vertical_resolution = self.gpu_stat.vertical_resolution();
        let is_interlace = self.gpu_stat.intersects(GpuStat::VERTICAL_INTERLACE);

        self.dot += 1;
        if self.dot >= max_dots {
            self.dot = 0;
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
    }

    fn clock_gp0_command(&mut self) {
        // try to empty the fifo
        while let Some(gp0_data) = self.command_fifo.pop_front() {
            log::info!("fifo len {}", self.command_fifo.len() + 1);
            if let Some(cmd) = self.current_command.as_mut() {
                // add the new data we received
                if cmd.still_need_params() {
                    log::info!("gp0 extra param {:08X}", gp0_data);
                    cmd.add_param(gp0_data);
                    if cmd.exec_command(&mut self.gpu_context) {
                        self.current_command = None;
                    }
                } else {
                    // put the data back
                    self.command_fifo.push_front(gp0_data);
                    break;
                }
            } else {
                log::info!("gp0 command {:08X} init", gp0_data);
                let mut cmd = instantiate_gp0_command(gp0_data);

                // if its not finished yet
                if !cmd.exec_command(&mut self.gpu_context) {
                    self.current_command = Some(cmd);
                }
            }
        }
        // clock the command even if no fifo is present
        if let Some(cmd) = self.current_command.as_mut() {
            if cmd.exec_command(&mut self.gpu_context) {
                self.current_command = None;
            }
        }
    }

    /// Some gp0 commands are executing even if the fifo is not empty, so we
    /// should bypass the fifo and execute them here
    fn execute_gp0_or_add_to_fifo(&mut self, data: u32) {
        let cmd = data >> 24;
        log::info!("gp0 command {:02X} data: {:08X}", cmd, data);
        // TODO: handle commands that bypass the fifo, like `0x00, 0xe3, 0xe4, 0xe5, etc.`
        match cmd {
            _ => {
                // add the command or param to the fifo
                self.command_fifo.push_back(data);
            }
        }
    }

    fn handle_gp0_input(&mut self, data: u32) {
        // if we still executing some command
        if let Some(cmd) = self.current_command.as_mut() {
            // add the new data we received
            if !cmd.still_need_params() {
                self.execute_gp0_or_add_to_fifo(data);
            } else {
                self.command_fifo.push_back(data);
            }
            return;
        }
        self.execute_gp0_or_add_to_fifo(data);
    }

    fn run_gp1_command(&mut self, data: u32) {
        let cmd = data >> 24;
        log::info!("gp1 command {:02X} data: {:08X}", cmd, data);
        match cmd {
            0x00 => {
                // Reset Gpu
                self.gpu_stat = GpuStat::empty();
                self.gpu_stat.insert(
                    GpuStat::DISPLAY_DISABLED
                        | GpuStat::INTERLACE_FIELD
                        | GpuStat::READY_FOR_DMA_RECV
                        | GpuStat::READY_FOR_CMD_RECV,
                );
            }
            0x01 => {
                // Reset command fifo buffer
                log::info!(
                    "Gpu resetting fifo, now at length={}",
                    self.command_fifo.len()
                );
                if let Some(cmd) = &mut self.current_command {
                    match cmd.cmd_type() {
                        Gp0CmdType::CpuToVramBlit => {
                            // flush vram write

                            // FIXME: close the write here and flush
                            //  do not add more data
                            while !cmd.exec_command(&mut self.gpu_context) {
                                if cmd.still_need_params() {
                                    cmd.add_param(0);
                                }
                            }
                            self.current_command = None;
                        }
                        _ => {}
                    }
                }
                self.command_fifo.clear();
            }
            0x02 => {
                // Reset IRQ
                self.gpu_stat.remove(GpuStat::INTERRUPT_REQUEST);
            }
            0x03 => {
                // Display enable
                self.gpu_stat.set(GpuStat::DISPLAY_DISABLED, data & 1 == 1)
            }
            0x04 => {
                // DMA direction
                self.gpu_stat.remove(GpuStat::DMA_DIRECTION);
                self.gpu_stat.bits |= (data & 3) << 29;

                // TODO: should also affect GpuStat::DMA_DATA_REQUEST
            }
            0x05 => {
                // Vram Start of Display area

                let x = data & 0x3ff;
                let y = (data >> 10) & 0x1ff;

                self.vram_display_area_start = (x, y);
                log::info!("vram display start area {:?}", self.vram_display_area_start);
            }
            0x06 => {
                // Screen Horizontal Display range
                let x1 = data & 0xfff;
                let x2 = (data >> 12) & 0xfff;

                self.display_horizontal_range = (x1, x2);
                log::info!(
                    "display horizontal range {:?}",
                    self.display_horizontal_range
                );
            }
            0x07 => {
                // Screen Vertical Display range
                let y1 = data & 0x1ff;
                let y2 = (data >> 10) & 0x1ff;

                self.display_vertical_range = (y1, y2);
                log::info!("display vertical range {:?}", self.display_vertical_range);
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

                self.gpu_stat.bits &= !0x7f6000;
                self.gpu_stat.bits |= stat_bits_17_22 << 17;
                self.gpu_stat.bits |= stat_bit_14_reverse_flag << 14;
                self.gpu_stat.bits |= stat_bit_16_horizontal_resolution_2 << 16;
                self.gpu_stat.bits |= interlace_field << 13;
            }
            0x09 => {
                // Allow texture disable
                self.allow_texture_disable = data & 1 == 1;
            }
            0x10 => {
                // GPU info

                // 0x0~0xF retreive info, and the rest are mirrors
                let info_id = data & 0xF;

                // make sure we are not overriding any data
                assert!(self.gpu_read.is_none());

                // TODO: some commands read old value of GPUREAD, we can't do that
                // now. might need to change how we handle GPUREAD in general
                let result = match info_id {
                    2 => {
                        // Read Texture Window setting GP0(E2h)
                        self.cached_gp0_e2
                    }
                    3 => {
                        // Read Draw area top left GP0(E3h)
                        self.cached_gp0_e3
                    }
                    4 => {
                        // Read Draw area bottom right GP0(E4h)
                        self.cached_gp0_e4
                    }
                    5 => {
                        // Read Draw offset GP0(E5h)
                        self.cached_gp0_e5
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

                self.gpu_read = Some(result);
            }
            _ => todo!("gp1 command {:02X}", cmd),
        }
    }
}

impl BusLine for Gpu {
    fn read_u32(&mut self, addr: u32) -> u32 {
        match addr {
            0 => self.gpu_read(),
            4 => self.gpu_stat(),
            _ => unreachable!(),
        }
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        match addr {
            0 => self.handle_gp0_input(data),
            4 => self.run_gp1_command(data),
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
