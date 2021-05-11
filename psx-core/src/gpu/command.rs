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
        5 => todo!(),
        6 => todo!(),
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
    colors: [u32; 4],
    vertices: [u32; 4],
    texture_data: [u32; 4],
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
            colors: [data0 & 0xFFFFFF, 0, 0, 0],
            vertices: [0; 4],
            texture_data: [0; 4],
            current_input_state: 1,
            input_pointer: 0,
        }
    }

    fn add_param(&mut self, param: u32) {
        match self.current_input_state {
            0 => {
                self.colors[self.input_pointer] = param & 0xFFFFFF;
                self.current_input_state = 1;
            }
            1 => {
                self.vertices[self.input_pointer] = param;
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
                self.texture_data[self.input_pointer] = param;
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

    fn exec_command(&mut self, _ctx: &mut GpuContext) -> bool {
        log::info!("POLYGON executing {:?}", self);

        if (self.input_pointer == 4 && self.is_4_vertices)
            || (!self.is_4_vertices && self.input_pointer == 3)
        {
            true
        } else {
            false
        }
    }

    fn still_need_params(&mut self) -> bool {
        todo!()
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
        todo!()
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
            _ => todo!("gp0 misc command {:02X}", cmd),
        }

        true
    }

    fn still_need_params(&mut self) -> bool {
        todo!()
    }
}
