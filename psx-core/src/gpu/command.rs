use super::gpu_context::{DrawingTextureParams, DrawingVertex};
use super::GpuContext;

// TODO: using dyn and dynamic dispatch might not be the best case for fast performance
//  we might need to change into another solution
pub fn instantiate_gp0_command(data: u32) -> Box<dyn Gp0Command> {
    let cmd = data >> 29;

    match cmd {
        0 => Box::new(MiscCommand::new(data)),
        1 => Box::new(PolygonCommand::new(data)),
        2 => todo!(),
        3 => todo!(),
        4 => todo!(),
        5 => Box::new(CpuToVramBlitCommand::new(data)),
        6 => Box::new(VramToCpuBlitCommand::new(data)),
        7 => Box::new(EnvironmentCommand::new(data)),
        _ => unreachable!(),
    }
}

pub trait Gp0Command {
    fn new(data0: u32) -> Self
    where
        Self: Sized;
    fn add_param(&mut self, param: u32);
    fn exec_command(&mut self, ctx: &mut GpuContext) -> bool;
    fn still_need_params(&mut self) -> bool;
}

#[derive(Debug)]
struct PolygonCommand {
    gouraud: bool,
    is_4_vertices: bool,
    textured: bool,
    semi_transparent: bool,
    texture_blending: bool,
    vertices: [DrawingVertex; 4],
    texture_params: DrawingTextureParams,
    current_input_state: u8,
    input_pointer: usize,
}

impl Gp0Command for PolygonCommand {
    fn new(data0: u32) -> Self
    where
        Self: Sized,
    {
        Self {
            gouraud: (data0 >> 28) & 1 == 1,
            is_4_vertices: (data0 >> 27) & 1 == 1,
            textured: (data0 >> 26) & 1 == 1,
            semi_transparent: (data0 >> 25) & 1 == 1,
            texture_blending: (data0 >> 24) & 1 == 1,
            vertices: [
                DrawingVertex::new_with_color(data0),
                DrawingVertex::new_with_color(data0),
                DrawingVertex::new_with_color(data0),
                DrawingVertex::new_with_color(data0),
            ],
            texture_params: DrawingTextureParams::default(),
            current_input_state: 1,
            input_pointer: 0,
        }
    }

    fn add_param(&mut self, param: u32) {
        match self.current_input_state {
            0 => {
                self.vertices[self.input_pointer].color_from_u32(param);
                self.current_input_state = 1;
            }
            1 => {
                self.vertices[self.input_pointer].position_from_u32(param);
                if self.textured {
                    self.current_input_state = 2;
                } else {
                    if self.gouraud {
                        self.current_input_state = 0;
                    }
                    self.input_pointer += 1;
                }
            }
            2 => {
                match self.input_pointer {
                    0 => self.texture_params.clut_from_u32(param),
                    1 => self.texture_params.tex_page_from_u32(param),
                    _ => {}
                }
                self.vertices[self.input_pointer].tex_coord_from_u32(param);
                if self.gouraud {
                    self.current_input_state = 0;
                } else {
                    self.current_input_state = 1;
                }
                self.input_pointer += 1;
            }
            _ => unreachable!(),
        }
    }

    fn exec_command(&mut self, ctx: &mut GpuContext) -> bool {
        if !self.still_need_params() {
            log::info!("POLYGON executing {:#?}", self);
            if self.semi_transparent || self.texture_blending {
                todo!()
            }

            ctx.draw_polygon(
                &self.vertices[..self.input_pointer],
                &self.texture_params,
                self.textured,
            );

            true
        } else {
            false
        }
    }

    fn still_need_params(&mut self) -> bool {
        !((self.input_pointer == 4 && self.is_4_vertices)
            || (self.input_pointer == 3 && !self.is_4_vertices))
    }
}

struct EnvironmentCommand(u32);

impl Gp0Command for EnvironmentCommand {
    fn new(data0: u32) -> Self
    where
        Self: Sized,
    {
        Self(data0)
    }

    fn add_param(&mut self, _param: u32) {
        unreachable!()
    }

    fn exec_command(&mut self, ctx: &mut GpuContext) -> bool {
        let data = self.0;
        let cmd = data >> 24;
        log::info!("gp0 command {:02X} data: {:08X}", cmd, data);
        match cmd {
            0xe1 => {
                // Draw Mode setting

                // 0-3   Texture page X Base   (N*64)
                // 4     Texture page Y Base   (N*256)
                // 5-6   Semi Transparency     (0=B/2+F/2, 1=B+F, 2=B-F, 3=B+F/4)
                // 7-8   Texture page colors   (0=4bit, 1=8bit, 2=15bit, 3=Reserved)
                // 9     Dither 24bit to 15bit (0=Off/strip LSBs, 1=Dither Enabled)
                // 10    Drawing to display area (0=Prohibited, 1=Allowed)
                // 11    Set Mask-bit when drawing pixels (0=No, 1=Yes/Mask)
                let stat_lower_11_bits = data & 0x7FF;
                let stat_bit_15_texture_disable = (data >> 11) & 1;

                #[allow(unused)]
                let textured_rect_x_flip = (data >> 12) & 1;
                #[allow(unused)]
                let textured_rect_y_flip = (data >> 13) & 1;

                ctx.gpu_stat.bits &= !0x87FF;
                ctx.gpu_stat.bits |= stat_lower_11_bits;
                ctx.gpu_stat.bits |= stat_bit_15_texture_disable << 15;
            }
            0xe2 => {
                // Texture window settings
                let mask_x = data & 0x1F;
                let mask_y = (data >> 5) & 0x1F;
                let offset_x = (data >> 10) & 0x1F;
                let offset_y = (data >> 15) & 0x1F;

                ctx.texture_window_mask = (mask_x, mask_y);
                ctx.texture_window_offset = (offset_x, offset_y);

                log::info!(
                    "texture window mask = {:?}, offset = {:?}",
                    ctx.texture_window_mask,
                    ctx.texture_window_offset
                );
            }
            0xe3 => {
                // Set Drawing Area top left
                let x = data & 0x3ff;
                let y = (data >> 10) & 0x3ff;
                ctx.drawing_area_top_left = (x, y);
                log::info!("drawing area top left = {:?}", ctx.drawing_area_top_left,);
            }
            0xe4 => {
                // Set Drawing Area bottom right
                let x = data & 0x3ff;
                let y = (data >> 10) & 0x3ff;
                ctx.drawing_area_bottom_right = (x, y);
                log::info!(
                    "drawing area bottom right = {:?}",
                    ctx.drawing_area_bottom_right,
                );
            }
            0xe5 => {
                // Set Drawing offset
                // TODO: test the accuracy of the sign extension
                let x = data & 0x7ff;
                let sign_extend = 0xfffff800 * ((x >> 10) & 1);
                let x = (x | sign_extend) as i32;
                let y = (data >> 11) & 0x7ff;
                let sign_extend = 0xfffff800 * ((y >> 10) & 1);
                let y = (y | sign_extend) as i32;
                ctx.drawing_offset = (x, y);
                log::info!("drawing offset = {:?}", ctx.drawing_offset,);
            }
            0xe6 => {
                // Mask Bit Setting

                //  11    Set mask while drawing (0=TextureBit15, 1=ForceBit15=1)
                //  12    Check mask before draw (0=Draw Always, 1=Draw if Bit15=0)
                let stat_bits_11_12 = data & 3;
                ctx.gpu_stat.bits &= !(3 << 11);
                ctx.gpu_stat.bits |= stat_bits_11_12;
            }
            _ => todo!("gp0 environment command {:02X}", cmd),
        }

        true
    }

    fn still_need_params(&mut self) -> bool {
        false
    }
}

struct MiscCommand(u32);

impl Gp0Command for MiscCommand {
    fn new(data0: u32) -> Self
    where
        Self: Sized,
    {
        Self(data0)
    }

    fn add_param(&mut self, _param: u32) {
        unreachable!()
    }

    fn exec_command(&mut self, _ctx: &mut GpuContext) -> bool {
        let data = self.0;
        let cmd = data >> 24;
        match cmd {
            0x00 => {
                // Nop
            }
            0x01 => {
                // Invalidate CLUT cache
            }
            _ => todo!("gp0 misc command {:02X}", cmd),
        }

        true
    }

    fn still_need_params(&mut self) -> bool {
        false
    }
}

struct CpuToVramBlitCommand {
    input_state: u8,
    start_dest: (u32, u32),
    next_dest: (u32, u32),
    start_size: (u32, u32),
    remaining_size: (u32, u32),
    next_data: Option<u32>,
}

impl Gp0Command for CpuToVramBlitCommand {
    fn new(_data0: u32) -> Self
    where
        Self: Sized,
    {
        Self {
            input_state: 0,
            start_size: (0, 0),
            remaining_size: (0, 0),
            next_data: None,
            start_dest: (0, 0),
            next_dest: (0, 0),
        }
    }

    fn add_param(&mut self, param: u32) {
        match self.input_state {
            0 => {
                let start_x = param & 0x3FF;
                let start_y = (param >> 16) & 0x1FF;
                self.start_dest = (start_x, start_y);
                self.next_dest = (start_x, start_y);
                log::info!("CPU to VRAM: input dest {:?}", self.start_dest);
                self.input_state = 1;
            }
            1 => {
                let size_x = ((param & 0xFFFF).wrapping_sub(1) & 0x3FF) + 1;
                let size_y = ((param >> 16).wrapping_sub(1) & 0x1FF) + 1;
                self.start_size = (size_x, size_y);
                self.remaining_size = (size_x, size_y);
                log::info!("CPU to VRAM: size {:?}", self.start_size);
                self.input_state = 2;
            }
            2 => self.next_data = Some(param),
            _ => unreachable!(),
        }
    }

    fn exec_command(&mut self, ctx: &mut GpuContext) -> bool {
        if let Some(next_data) = self.next_data.take() {
            let (x_counter, y_counter) = &mut self.remaining_size;

            log::info!(
                "IN TRANSFERE, dest={:?}, data={:08X}",
                self.next_dest,
                next_data
            );
            let d1 = next_data as u16;
            let d2 = (next_data >> 16) as u16;

            for data in &[d1, d2] {
                ctx.write_vram_checked(self.next_dest, *data);

                self.next_dest.0 = (self.next_dest.0 + 1) & 0x3FF;
                *x_counter -= 1;

                if *x_counter == 0 {
                    self.next_dest.1 = (self.next_dest.1 + 1) & 0x1FF;

                    *y_counter -= 1;
                    *x_counter = self.start_size.0;
                    self.next_dest.0 = self.start_dest.0;

                    if *y_counter == 0 {
                        // finish transfer
                        log::info!("DONE TRANSFERE");
                        // update the texture buffer from the vram when we finish
                        // writing to vram
                        ctx.update_texture_buffer();
                        return true;
                    }
                }
            }
        }

        false
    }

    fn still_need_params(&mut self) -> bool {
        // still inputtning header (state not 3) or we still have some rows
        self.input_state != 2 || self.remaining_size.1 > 0
    }
}

struct VramToCpuBlitCommand {
    input_state: u8,
    src: (u32, u32),
    size: (u32, u32),
    block: Vec<u16>,
    block_counter: usize,
}

impl Gp0Command for VramToCpuBlitCommand {
    fn new(_data0: u32) -> Self
    where
        Self: Sized,
    {
        Self {
            input_state: 0,
            size: (0, 0),
            src: (0, 0),
            block: Vec::new(),
            block_counter: 0,
        }
    }

    fn add_param(&mut self, param: u32) {
        match self.input_state {
            0 => {
                let start_x = param & 0x3FF;
                let start_y = (param >> 16) & 0x1FF;
                self.src = (start_x, start_y);
                log::info!("VRAM to CPU: input src {:?}", self.src);
                self.input_state = 1;
            }
            1 => {
                let size_x = ((param & 0xFFFF).wrapping_sub(1) & 0x3FF) + 1;
                let size_y = ((param >> 16).wrapping_sub(1) & 0x1FF) + 1;
                self.size = (size_x, size_y);
                log::info!("VRAM to CPU: size {:?}", self.size);
                self.input_state = 2;
            }
            _ => unreachable!(),
        }
    }

    fn exec_command(&mut self, ctx: &mut GpuContext) -> bool {
        if self.input_state != 2 || ctx.gpu_read.is_some() {
            return false;
        }
        if self.block.is_empty() {
            let x_range = (self.src.0)..(self.src.0 + self.size.0);
            let y_range = (self.src.1)..(self.src.1 + self.size.1);

            self.block = ctx.read_vram_block((x_range, y_range));
        }

        // used for debugging only
        let vram_pos = (
            (self.block_counter as u32 % self.size.0) + self.src.0,
            (self.block_counter as u32 / self.size.0) + self.src.1,
        );
        let data_parts = &self.block[self.block_counter..(self.block_counter + 2)];
        self.block_counter += 2;

        // TODO: check order
        let data = ((data_parts[1] as u32) << 16) | data_parts[0] as u32;
        log::info!("IN TRANSFERE, src={:?}, data={:08X}", vram_pos, data);

        ctx.gpu_read = Some(data);

        if self.block_counter == self.block.len() {
            // finish transfer
            log::info!("DONE TRANSFERE");
            true
        } else {
            false
        }
    }

    fn still_need_params(&mut self) -> bool {
        // still inputting header (state not 3) or we still have some rows
        self.input_state != 2
    }
}
