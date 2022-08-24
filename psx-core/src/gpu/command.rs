use crate::gpu::GpuStat;

use super::gpu_context::{vertex_position_from_u32, DrawingTextureParams, DrawingVertex};
use super::GpuContext;

#[derive(Debug)]
pub enum Gp0CmdType {
    Misc = 0,
    Polygon = 1,
    Line = 2,
    Rectangle = 3,
    VramToVramBlit = 4,
    CpuToVramBlit = 5,
    VramToCpuBlit = 6,
    Environment = 7,
    // the `cmd` is actually `0`, but only one can have only one which is zero
    FillVram = 8,
}

// TODO: using dyn and dynamic dispatch might not be the best case for fast performance
//  we might need to change into another solution
pub fn instantiate_gp0_command(data: u32) -> Box<dyn Gp0Command> {
    let cmd = data >> 29;

    match cmd {
        0 => match data >> 24 {
            0x02 => Box::new(FillVramCommand::new(data)),
            _ => Box::new(MiscCommand::new(data)),
        },
        1 => Box::new(PolygonCommand::new(data)),
        2 => Box::new(LineCommand::new(data)),
        3 => Box::new(RectangleCommand::new(data)),
        4 => Box::new(VramToVramBlitCommand::new(data)),
        5 => Box::new(CpuToVramBlitCommand::new(data)),
        6 => Box::new(VramToCpuBlitCommand::new(data)),
        7 => Box::new(EnvironmentCommand::new(data)),
        _ => unreachable!(),
    }
}

/// Commands constructed in the frontend and sent to the gpu on the backend
/// for rendering.
pub trait Gp0Command: Send {
    fn new(data0: u32) -> Self
    where
        Self: Sized;
    fn add_param(&mut self, param: u32);
    fn exec_command(&mut self, ctx: &mut GpuContext);
    fn still_need_params(&mut self) -> bool;
    fn cmd_type(&self) -> Gp0CmdType;
}

#[derive(Debug)]
struct PolygonCommand {
    gouraud: bool,
    is_4_vertices: bool,
    textured: bool,
    semi_transparent: bool,
    texture_blending: bool,
    vertices: [DrawingVertex; 6],
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
            texture_blending: (data0 >> 24) & 1 == 0, // enabled with 0
            vertices: [DrawingVertex::new_with_color(data0); 6],
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

    fn exec_command(&mut self, ctx: &mut GpuContext) {
        assert!(!self.still_need_params());
        log::info!("POLYGON executing {:#?}", self);

        let input_pointer = if self.is_4_vertices {
            self.vertices[4] = self.vertices[2];
            self.vertices[5] = self.vertices[1];
            6
        } else {
            3
        };
        ctx.draw_polygon(
            &self.vertices[..input_pointer],
            self.texture_params,
            self.textured,
            self.texture_blending,
            self.semi_transparent,
        );
    }

    fn still_need_params(&mut self) -> bool {
        !((self.input_pointer == 4 && self.is_4_vertices)
            || (self.input_pointer == 3 && !self.is_4_vertices))
    }

    fn cmd_type(&self) -> Gp0CmdType {
        Gp0CmdType::Polygon
    }
}

#[derive(Debug)]
struct LineCommand {
    gouraud: bool,
    polyline: bool,
    semi_transparent: bool,
    first_color: u32,
    vertices: Vec<DrawingVertex>,
    expecting_vertex_position: bool,
    done_input: bool,
    polyline_can_be_finished: bool,
}

impl Gp0Command for LineCommand {
    fn new(data0: u32) -> Self
    where
        Self: Sized,
    {
        let gouraud = (data0 >> 28) & 1 == 1;
        let polyline = (data0 >> 27) & 1 == 1;
        let vertices = if gouraud {
            vec![DrawingVertex::new_with_color(data0)]
        } else {
            Vec::new()
        };
        Self {
            gouraud,
            polyline,
            semi_transparent: (data0 >> 25) & 1 == 1,
            first_color: data0 & 0xFFFFFF,
            vertices,
            expecting_vertex_position: true,
            done_input: false,
            polyline_can_be_finished: false,
        }
    }

    fn add_param(&mut self, param: u32) {
        // end of polyline
        if self.polyline
            && self.polyline_can_be_finished
            && self.vertices.len() >= 2
            && (param & 0xF000F000) == 0x50005000
        {
            self.done_input = true;
            return;
        }

        if self.expecting_vertex_position {
            if self.gouraud {
                self.vertices.last_mut().unwrap().position_from_u32(param);
                self.expecting_vertex_position = false;
            } else {
                if self.polyline && self.vertices.len() >= 2 {
                    // duplicate the last vertex
                    self.vertices.push(*self.vertices.last().unwrap());
                }
                let mut vertex = DrawingVertex::new_with_color(self.first_color);
                vertex.position_from_u32(param);
                self.vertices.push(vertex);
            }
            self.polyline_can_be_finished = true;
            if !self.polyline && self.vertices.len() == 2 {
                self.done_input = true;
            }
        } else {
            if self.polyline && self.vertices.len() >= 2 {
                // duplicate the last vertex
                self.vertices.push(*self.vertices.last().unwrap());
            }
            self.vertices.push(DrawingVertex::new_with_color(param));
            self.expecting_vertex_position = true;
            self.polyline_can_be_finished = false;
        }
    }

    fn exec_command(&mut self, ctx: &mut GpuContext) {
        assert!(!self.still_need_params());
        log::info!("LINE executing {:#?}", self);

        ctx.draw_polyline(&self.vertices[..], self.semi_transparent);
    }

    fn still_need_params(&mut self) -> bool {
        !self.done_input
    }

    fn cmd_type(&self) -> Gp0CmdType {
        Gp0CmdType::Line
    }
}

#[derive(Debug)]
struct RectangleCommand {
    textured: bool,
    semi_transparent: bool,
    size_mode: u8,
    size: [f32; 2],
    vertices: [DrawingVertex; 6],
    texture_params: DrawingTextureParams,
    current_input_state: u8,
}

impl Gp0Command for RectangleCommand {
    fn new(data0: u32) -> Self
    where
        Self: Sized,
    {
        Self {
            size_mode: ((data0 >> 27) & 3) as u8,
            size: [0.0; 2],
            textured: (data0 >> 26) & 1 == 1,
            semi_transparent: (data0 >> 25) & 1 == 1,
            vertices: [DrawingVertex::new_with_color(data0); 6],
            texture_params: DrawingTextureParams::default(),
            current_input_state: 0,
        }
    }

    fn add_param(&mut self, param: u32) {
        match self.current_input_state {
            0 => {
                // vertex1 input
                self.vertices[0].position_from_u32(param);
                if self.textured {
                    self.current_input_state = 1;
                } else {
                    // variable size
                    if self.size_mode == 0 {
                        self.current_input_state = 2;
                    } else {
                        self.current_input_state = 3;
                    }
                }
            }
            1 => {
                // texture data input
                self.texture_params.clut_from_u32(param);
                self.vertices[0].tex_coord_from_u32(param);

                // variable size
                if self.size_mode == 0 {
                    self.current_input_state = 2;
                } else {
                    self.current_input_state = 3;
                }
            }
            2 => {
                // variable size input
                self.size = vertex_position_from_u32(param);
                self.current_input_state = 3;
            }
            _ => unreachable!(),
        }
    }

    fn exec_command(&mut self, ctx: &mut GpuContext) {
        // TODO: Add texture repeat of U,V exceed 255
        assert!(!self.still_need_params());
        // compute the location of other vertices
        let top_left = self.vertices[0].position();
        let top_left_tex = self.vertices[0].tex_coord();
        let size = match self.size_mode {
            0 => &self.size,
            1 => &[1.0; 2],
            2 => &[8.0; 2],
            3 => &[16.0; 2],
            _ => unreachable!(),
        };
        let size_coord = [size[0] as i32, size[1] as i32];

        // top right
        // NOTE: for some reason, -1 when computing tex coords is needed
        //  check if its true or not.
        self.vertices[1].set_position([top_left[0] + size[0], top_left[1]]);
        self.vertices[1].set_tex_coord([
            (top_left_tex[0] as i32 + size_coord[0] - 1).min(255).max(0) as u32,
            top_left_tex[1],
        ]);
        // bottom left
        self.vertices[2].set_position([top_left[0], top_left[1] + size[1]]);
        self.vertices[2].set_tex_coord([
            top_left_tex[0],
            (top_left_tex[1] as i32 + size_coord[1] - 1).min(255).max(0) as u32,
        ]);
        // copies of top right and bottom left for the second triangle
        self.vertices[3] = self.vertices[1];
        self.vertices[4] = self.vertices[2];

        // bottom right
        self.vertices[5].set_position([top_left[0] + size[0], top_left[1] + size[1]]);
        self.vertices[5].set_tex_coord([
            (top_left_tex[0] as i32 + size_coord[0] - 1).min(255).max(0) as u32,
            (top_left_tex[1] as i32 + size_coord[1] - 1).min(255).max(0) as u32,
        ]);

        log::info!("RECTANGLE executing {:#?}", self);
        if self.textured {
            // it will just take what is needed from the stat, which include the tex page
            // to use and color mode
            self.texture_params
                .tex_page_from_gpustat(ctx.read_gpu_stat().bits);
            self.texture_params.set_texture_flip(ctx.textured_rect_flip);
        }

        ctx.draw_polygon(
            &self.vertices,
            self.texture_params,
            self.textured,
            false,
            self.semi_transparent,
        );
    }

    fn still_need_params(&mut self) -> bool {
        self.current_input_state != 3
    }

    fn cmd_type(&self) -> Gp0CmdType {
        Gp0CmdType::Rectangle
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

    fn exec_command(&mut self, ctx: &mut GpuContext) {
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
                // 11    Texture Disable (0=Normal, 1=Disable if GP1(09h).Bit0=1)   ;GPUSTAT.15
                let stat_lower_11_bits = data & 0x7FF;
                let stat_bit_15_texture_disable = (data >> 11) & 1 == 1;

                let textured_rect_x_flip = (data >> 12) & 1 == 1;
                let textured_rect_y_flip = (data >> 13) & 1 == 1;
                ctx.textured_rect_flip = (textured_rect_x_flip, textured_rect_y_flip);

                ctx.gpu_stat
                    .fetch_update(|mut s| {
                        s.bits &= !0x87FF;
                        s.bits |= stat_lower_11_bits;
                        if stat_bit_15_texture_disable && ctx.allow_texture_disable {
                            s.bits |= 1 << 15;
                        }
                        Some(s)
                    })
                    .unwrap();
            }
            0xe2 => {
                ctx.cached_gp0_e2 = data;

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
                ctx.cached_gp0_e3 = data;

                // Set Drawing Area top left
                let x = data & 0x3ff;
                let y = (data >> 10) & 0x3ff;
                ctx.drawing_area_top_left = (x, y);
                log::info!("drawing area top left = {:?}", ctx.drawing_area_top_left,);
            }
            0xe4 => {
                ctx.cached_gp0_e4 = data;

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
                ctx.cached_gp0_e5 = data;

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

                ctx.gpu_stat
                    .fetch_update(|mut s| {
                        s.bits &= !(3 << 11);
                        s.bits |= stat_bits_11_12;
                        Some(s)
                    })
                    .unwrap();
            }
            _ => todo!("gp0 environment command {:02X}", cmd),
        }
    }

    fn still_need_params(&mut self) -> bool {
        false
    }

    fn cmd_type(&self) -> Gp0CmdType {
        Gp0CmdType::Environment
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

    fn exec_command(&mut self, _ctx: &mut GpuContext) {
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
    }

    fn still_need_params(&mut self) -> bool {
        false
    }

    fn cmd_type(&self) -> Gp0CmdType {
        Gp0CmdType::Misc
    }
}

struct CpuToVramBlitCommand {
    input_state: u8,
    dest: (u32, u32),
    size: (u32, u32),
    total_size: usize,

    block: Vec<u16>,
}

impl Gp0Command for CpuToVramBlitCommand {
    fn new(_data0: u32) -> Self
    where
        Self: Sized,
    {
        Self {
            input_state: 0,
            size: (0, 0),
            total_size: 0,
            dest: (0, 0),

            block: Vec::new(),
        }
    }

    fn add_param(&mut self, param: u32) {
        match self.input_state {
            0 => {
                let start_x = param & 0x3FF;
                let start_y = (param >> 16) & 0x1FF;
                self.dest = (start_x, start_y);
                log::info!("CPU to VRAM: input dest {:?}", self.dest);
                self.input_state = 1;
            }
            1 => {
                let size_x = ((param & 0xFFFF).wrapping_sub(1) & 0x3FF) + 1;
                let size_y = ((param >> 16).wrapping_sub(1) & 0x1FF) + 1;
                self.size = (size_x, size_y);
                self.total_size = (size_x * size_y) as usize;

                // we add one, so that if the size is odd, we would not need to
                // re-allocate the block when we push the last value.
                self.block.reserve(self.total_size + 1);
                log::info!("CPU to VRAM: size {:?}", self.size);
                self.input_state = 2;
            }
            2 => {
                // for debugging
                let vram_pos = (
                    (self.block.len() as u32 % self.size.0) + self.dest.0,
                    (self.block.len() as u32 / self.size.0) + self.dest.1,
                );
                log::info!("IN TRANSFERE, dest={:?}, data={:08X}", vram_pos, param);

                let d1 = param as u16;
                let d2 = (param >> 16) as u16;

                self.block.push(d1);
                self.block.push(d2);
            }
            _ => unreachable!(),
        }
    }

    fn exec_command(&mut self, ctx: &mut GpuContext) {
        assert!(!self.still_need_params());

        let x_range = (self.dest.0)..(self.dest.0 + self.size.0);
        let y_range = (self.dest.1)..(self.dest.1 + self.size.1);

        ctx.write_vram_block((x_range, y_range), &self.block[..self.total_size]);
    }

    fn still_need_params(&mut self) -> bool {
        self.input_state != 2 || self.block.len() < self.total_size
    }

    fn cmd_type(&self) -> Gp0CmdType {
        Gp0CmdType::CpuToVramBlit
    }
}

struct VramToVramBlitCommand {
    input_state: u8,
    src: (u32, u32),
    dest: (u32, u32),
    size: (u32, u32),
}

impl Gp0Command for VramToVramBlitCommand {
    fn new(_data0: u32) -> Self
    where
        Self: Sized,
    {
        Self {
            input_state: 0,
            src: (0, 0),
            dest: (0, 0),
            size: (0, 0),
        }
    }

    fn add_param(&mut self, param: u32) {
        match self.input_state {
            0 => {
                let start_x = param & 0x3FF;
                let start_y = (param >> 16) & 0x1FF;
                self.src = (start_x, start_y);
                log::info!("VRAM to VRAM: input src {:?}", self.src);
                self.input_state = 1;
            }
            1 => {
                let start_x = param & 0x3FF;
                let start_y = (param >> 16) & 0x1FF;
                self.dest = (start_x, start_y);
                log::info!("VRAM to VRAM: input dest {:?}", self.dest);
                self.input_state = 2;
            }
            2 => {
                let size_x = ((param & 0xFFFF).wrapping_sub(1) & 0x3FF) + 1;
                let size_y = ((param >> 16).wrapping_sub(1) & 0x1FF) + 1;
                self.size = (size_x, size_y);
                log::info!("VRAM to VRAM: size {:?}", self.size);
                self.input_state = 3;
            }
            _ => unreachable!(),
        }
    }

    fn exec_command(&mut self, ctx: &mut GpuContext) {
        assert!(!self.still_need_params());

        // TODO: use vulkan image copy itself
        let x_range = (self.src.0)..(self.src.0 + self.size.0);
        let y_range = (self.src.1)..(self.src.1 + self.size.1);
        let block = ctx.read_vram_block(&(x_range, y_range));

        let x_range = (self.dest.0)..(self.dest.0 + self.size.0);
        let y_range = (self.dest.1)..(self.dest.1 + self.size.1);
        ctx.write_vram_block((x_range, y_range), block.as_ref());
    }

    fn still_need_params(&mut self) -> bool {
        self.input_state != 3
    }

    fn cmd_type(&self) -> Gp0CmdType {
        Gp0CmdType::VramToVramBlit
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

    fn exec_command(&mut self, ctx: &mut GpuContext) {
        assert!(!self.still_need_params());
        assert!(self.block.is_empty());

        let x_range = (self.src.0)..(self.src.0 + self.size.0);
        let y_range = (self.src.1)..(self.src.1 + self.size.1);

        self.block = ctx.read_vram_block(&(x_range, y_range));

        while self.block_counter < self.block.len() {
            // used for debugging only
            let vram_pos = (
                (self.block_counter as u32 % self.size.0) + self.src.0,
                (self.block_counter as u32 / self.size.0) + self.src.1,
            );
            let d1 = self.block[self.block_counter];
            let d2 = if self.block_counter + 1 < self.block.len() {
                self.block[self.block_counter + 1]
            } else {
                0
            };
            self.block_counter += 2;

            let data = ((d2 as u32) << 16) | d1 as u32;
            log::info!("IN TRANSFERE, src={:?}, data={:08X}", vram_pos, data);

            // TODO: send full block
            ctx.send_to_gpu_read(data);
        }
        // after sending all the data, we set the gpu_stat bit to indicate that
        // the data can be read now
        ctx.gpu_stat
            .fetch_update(|s| Some(s | GpuStat::READY_FOR_TO_SEND_VRAM))
            .unwrap();
        log::info!("DONE TRANSFERE");
    }

    fn still_need_params(&mut self) -> bool {
        // still inputting header (state not 3) or we still have some rows
        self.input_state != 2
    }

    fn cmd_type(&self) -> Gp0CmdType {
        Gp0CmdType::VramToCpuBlit
    }
}

struct FillVramCommand {
    input_state: u8,
    color: (u8, u8, u8),
    top_left: (u32, u32),
    size: (u32, u32),
}

impl Gp0Command for FillVramCommand {
    fn new(data0: u32) -> Self
    where
        Self: Sized,
    {
        let r = (data0 & 0xFF) as u8;
        let g = ((data0 >> 8) & 0xFF) as u8;
        let b = ((data0 >> 16) & 0xFF) as u8;

        Self {
            input_state: 0,
            top_left: (0, 0),
            size: (0, 0),
            color: (r, g, b),
        }
    }

    fn add_param(&mut self, param: u32) {
        match self.input_state {
            0 => {
                let start_x = param & 0x3F0;
                let start_y = (param >> 16) & 0x1FF;
                self.top_left = (start_x, start_y);

                log::info!(
                    "Fill Vram: top_left {:?}, color {:?}",
                    self.top_left,
                    self.color
                );
                self.input_state = 1;
            }
            1 => {
                let size_x = ((param & 0x3FF) + 0xF) & !0xF;
                let size_y = (param >> 16) & 0x1FF;
                self.size = (size_x, size_y);
                log::info!("Fill Vram: size {:?}", self.size);
                self.input_state = 2;
            }
            _ => unreachable!(),
        }
    }

    fn exec_command(&mut self, ctx: &mut GpuContext) {
        assert!(!self.still_need_params());

        ctx.fill_color(self.top_left, self.size, self.color);
        log::info!("Fill Vram: done");
    }

    fn still_need_params(&mut self) -> bool {
        // still inputting header (state not 3) or we still have some rows
        self.input_state != 2
    }

    fn cmd_type(&self) -> Gp0CmdType {
        Gp0CmdType::FillVram
    }
}
