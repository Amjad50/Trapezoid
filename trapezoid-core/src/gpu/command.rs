use std::sync::Arc;

use super::{AtomicGpuStat, BackendCommand, GpuStat, GpuStateSnapshot};
use crate::gpu::common::{vertex_position_from_u32, DrawingTextureParams, DrawingVertex};

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
pub(super) fn instantiate_gp0_command(data: u32) -> Box<dyn Gp0Command> {
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
pub(super) trait Gp0Command: Send {
    fn new(data0: u32) -> Self
    where
        Self: Sized;
    fn add_param(&mut self, param: u32);
    fn exec_command(
        self: Box<Self>,
        gpu_stat: Arc<AtomicGpuStat>,
        state_snapshot: &mut GpuStateSnapshot,
    ) -> Option<BackendCommand>;
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

    fn exec_command(
        mut self: Box<Self>,
        gpu_stat: Arc<AtomicGpuStat>,
        state_snapshot: &mut GpuStateSnapshot,
    ) -> Option<BackendCommand> {
        assert!(!self.still_need_params());
        log::info!("POLYGON executing {:#?}", self);

        let input_pointer = if self.is_4_vertices {
            self.vertices[4] = self.vertices[2];
            self.vertices[5] = self.vertices[1];
            6
        } else {
            3
        };

        if self.textured {
            if !state_snapshot.allow_texture_disable {
                self.texture_params.texture_disable = false;
            }
            gpu_stat
                .fetch_update(|mut s| {
                    s.update_from_texture_params(&self.texture_params);
                    Some(s)
                })
                .unwrap();
        }
        state_snapshot.gpu_stat = gpu_stat.load();
        Some(BackendCommand::DrawPolygon {
            vertices: self.vertices[..input_pointer].to_vec(),
            texture_params: self.texture_params,
            textured: self.textured,
            texture_blending: self.texture_blending,
            semi_transparent: self.semi_transparent,
            state_snapshot: state_snapshot.clone(),
        })
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

    fn exec_command(
        mut self: Box<Self>,
        gpu_stat: Arc<AtomicGpuStat>,
        state_snapshot: &mut GpuStateSnapshot,
    ) -> Option<BackendCommand> {
        assert!(!self.still_need_params());
        log::info!("LINE executing {:#?}", self);

        state_snapshot.gpu_stat = gpu_stat.load();
        Some(BackendCommand::DrawPolyline {
            vertices: self.vertices,
            semi_transparent: self.semi_transparent,
            state_snapshot: state_snapshot.clone(),
        })
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
    texture_blending: bool,
    size_mode: u8,
    size: [i32; 2],
    vertices: Vec<DrawingVertex>,
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
            size: [0; 2],
            textured: (data0 >> 26) & 1 == 1,
            semi_transparent: (data0 >> 25) & 1 == 1,
            texture_blending: (data0 >> 24) & 1 == 0, // enabled with 0
            vertices: vec![DrawingVertex::new_with_color(data0); 6],
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
                let s = vertex_position_from_u32(param);
                self.size = [s[0] as i32, s[1] as i32];
                self.current_input_state = 3;
            }
            _ => unreachable!(),
        }
    }

    fn exec_command(
        mut self: Box<Self>,
        gpu_stat: Arc<AtomicGpuStat>,
        state_snapshot: &mut GpuStateSnapshot,
    ) -> Option<BackendCommand> {
        assert!(!self.still_need_params());

        // compute the location of other vertices
        let top_left = self.vertices[0].position();
        let top_left_tex = self.vertices[0].tex_coord();
        let size = match self.size_mode {
            0 => &self.size,
            1 => &[1; 2],
            2 => &[8; 2],
            3 => &[16; 2],
            _ => unreachable!(),
        };
        if size[0] == 0 || size[1] == 0 {
            return None; // empty rect
        }
        // The tex_coords, are large i32 numbers, they can be negative or more
        // than 255, and the shader will handle repeating and flipping based on the values.
        let size_f32 = [size[0] as f32, size[1] as f32];
        let mut bottom_right_tex = [top_left_tex[0], top_left_tex[1]];
        if state_snapshot.textured_rect_flip.0 {
            bottom_right_tex[0] -= size[0] - 1;
        } else {
            bottom_right_tex[0] += size[0];
        }
        if state_snapshot.textured_rect_flip.1 {
            bottom_right_tex[1] -= size[1] - 1;
        } else {
            bottom_right_tex[1] += size[1];
        }

        // top right
        self.vertices[1].set_position([top_left[0] + size_f32[0], top_left[1]]);
        self.vertices[1].set_tex_coord([bottom_right_tex[0], top_left_tex[1]]);
        // bottom left
        self.vertices[2].set_position([top_left[0], top_left[1] + size_f32[1]]);
        self.vertices[2].set_tex_coord([top_left_tex[0], bottom_right_tex[1]]);
        // copies of top right and bottom left for the second triangle
        self.vertices[3] = self.vertices[1];
        self.vertices[4] = self.vertices[2];

        // bottom right
        self.vertices[5].set_position([top_left[0] + size_f32[0], top_left[1] + size_f32[1]]);
        self.vertices[5].set_tex_coord(bottom_right_tex);

        if self.textured {
            // it will just take what is needed from the stat, which include the tex page
            // to use and color mode
            self.texture_params
                .tex_page_from_gpustat(gpu_stat.load().bits());

            if !state_snapshot.allow_texture_disable {
                self.texture_params.texture_disable = false;
            }
            gpu_stat
                .fetch_update(|mut s| {
                    s.update_from_texture_params(&self.texture_params);
                    Some(s)
                })
                .unwrap();
        }
        log::info!("RECTANGLE executing {:#?}", self);

        state_snapshot.gpu_stat = gpu_stat.load();
        Some(BackendCommand::DrawPolygon {
            vertices: self.vertices,
            texture_params: self.texture_params,
            textured: self.textured,
            texture_blending: self.texture_blending,
            semi_transparent: self.semi_transparent,
            state_snapshot: state_snapshot.clone(),
        })
    }

    fn still_need_params(&mut self) -> bool {
        self.current_input_state != 3
    }

    fn cmd_type(&self) -> Gp0CmdType {
        Gp0CmdType::Rectangle
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

    fn exec_command(
        self: Box<Self>,
        _gpu_stat: Arc<AtomicGpuStat>,
        _state_snapshot: &mut GpuStateSnapshot,
    ) -> Option<BackendCommand> {
        let data = self.0;
        let cmd = data >> 24;
        match cmd {
            0x00 | 0x03..=0x1E => {
                // Nop
            }
            0x01 => {
                // Invalidate CLUT cache
            }
            _ => todo!("gp0 misc command {:02X}", cmd),
        }
        None
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

                self.block.reserve(self.total_size);
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
                if self.block.len() < self.total_size {
                    self.block.push(d2);
                }
            }
            _ => unreachable!(),
        }
    }

    fn exec_command(
        mut self: Box<Self>,
        _gpu_stat: Arc<AtomicGpuStat>,
        _state_snapshot: &mut GpuStateSnapshot,
    ) -> Option<BackendCommand> {
        // command was executed normally
        if !self.still_need_params() {
            let x_range = (self.dest.0)..(self.dest.0 + self.size.0);
            let y_range = (self.dest.1)..(self.dest.1 + self.size.1);

            Some(BackendCommand::WriteVramBlock {
                block_range: (x_range, y_range),
                block: self.block,
            })
        } else {
            // command was aborted in the middle, let's just transfer the data we have
            if self.block.is_empty() {
                return None;
            }

            // we haven't finished a single row
            if self.block.len() < self.size.0 as usize {
                let x_range = (self.dest.0)..(self.dest.0 + self.block.len() as u32);
                let y_range = (self.dest.1)..(self.dest.1 + 1);

                Some(BackendCommand::WriteVramBlock {
                    block_range: (x_range, y_range),
                    block: self.block,
                })
            } else {
                // FIXME: we are sending only the full rows now and discarding the rest
                let n_rows = self.block.len() / self.size.0 as usize;
                let x_range = (self.dest.0)..(self.dest.0 + self.size.0);
                let y_range = (self.dest.1)..(self.dest.1 + n_rows as u32);

                Some(BackendCommand::WriteVramBlock {
                    block_range: (x_range, y_range),
                    block: self.block[..(n_rows * self.size.0 as usize)].to_vec(),
                })
            }
        }
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

    fn exec_command(
        mut self: Box<Self>,
        _gpu_stat: Arc<AtomicGpuStat>,
        _state_snapshot: &mut GpuStateSnapshot,
    ) -> Option<BackendCommand> {
        assert!(!self.still_need_params());

        let x_range = (self.src.0)..(self.src.0 + self.size.0);
        let y_range = (self.src.1)..(self.src.1 + self.size.1);
        let src = (x_range, y_range);

        let x_range = (self.dest.0)..(self.dest.0 + self.size.0);
        let y_range = (self.dest.1)..(self.dest.1 + self.size.1);
        let dst = (x_range, y_range);
        Some(BackendCommand::VramVramBlit { src, dst })
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

    fn exec_command(
        mut self: Box<Self>,
        _gpu_stat: Arc<AtomicGpuStat>,
        _state_snapshot: &mut GpuStateSnapshot,
    ) -> Option<BackendCommand> {
        assert!(!self.still_need_params());

        let x_range = (self.src.0)..(self.src.0 + self.size.0);
        let y_range = (self.src.1)..(self.src.1 + self.size.1);

        Some(BackendCommand::VramReadBlock {
            block_range: (x_range, y_range),
        })
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

    fn exec_command(
        mut self: Box<Self>,
        _gpu_stat: Arc<AtomicGpuStat>,
        _state_snapshot: &mut GpuStateSnapshot,
    ) -> Option<BackendCommand> {
        assert!(!self.still_need_params());

        Some(BackendCommand::FillColor {
            top_left: self.top_left,
            size: self.size,
            color: self.color,
        })
    }

    fn still_need_params(&mut self) -> bool {
        // still inputting header (state not 3) or we still have some rows
        self.input_state != 2
    }

    fn cmd_type(&self) -> Gp0CmdType {
        Gp0CmdType::FillVram
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

    fn exec_command(
        self: Box<Self>,
        gpu_stat: Arc<AtomicGpuStat>,
        state_snapshot: &mut GpuStateSnapshot,
    ) -> Option<BackendCommand> {
        let data = self.0;
        let cmd = data >> 24;
        log::info!("gp0 command {:02X} data: {:08X}", cmd, data);
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
                state_snapshot.textured_rect_flip = (textured_rect_x_flip, textured_rect_y_flip);

                gpu_stat
                    .fetch_update(|mut s| {
                        s &= GpuStat::from_bits_retain(!0x87FF);
                        s |= GpuStat::from_bits_retain(stat_lower_11_bits);
                        if stat_bit_15_texture_disable && state_snapshot.allow_texture_disable {
                            s |= GpuStat::DISABLE_TEXTURE; // 1 << 15
                        }
                        Some(s)
                    })
                    .unwrap();
            }
            0xe2 => {
                state_snapshot.cached_gp0_e2 = data;

                // Texture window settings
                let mask_x = data & 0x1F;
                let mask_y = (data >> 5) & 0x1F;
                let offset_x = (data >> 10) & 0x1F;
                let offset_y = (data >> 15) & 0x1F;

                state_snapshot.texture_window_mask = (mask_x, mask_y);
                state_snapshot.texture_window_offset = (offset_x, offset_y);

                log::info!(
                    "texture window mask = {:?}, offset = {:?}",
                    state_snapshot.texture_window_mask,
                    state_snapshot.texture_window_offset
                );
            }
            0xe3 => {
                state_snapshot.cached_gp0_e3 = data;

                // Set Drawing Area top left
                let x = data & 0x3ff;
                let y = (data >> 10) & 0x3ff;
                state_snapshot.drawing_area_top_left = (x, y);
                log::info!(
                    "drawing area top left = {:?}",
                    state_snapshot.drawing_area_top_left,
                );
            }
            0xe4 => {
                state_snapshot.cached_gp0_e4 = data;

                // Set Drawing Area bottom right
                let x = data & 0x3ff;
                let y = (data >> 10) & 0x3ff;
                state_snapshot.drawing_area_bottom_right = (x, y);
                log::info!(
                    "drawing area bottom right = {:?}",
                    state_snapshot.drawing_area_bottom_right,
                );
            }
            0xe5 => {
                state_snapshot.cached_gp0_e5 = data;

                // Set Drawing offset
                // TODO: test the accuracy of the sign extension
                let x = data & 0x7ff;
                let sign_extend = 0xfffff800 * ((x >> 10) & 1);
                let x = (x | sign_extend) as i32;
                let y = (data >> 11) & 0x7ff;
                let sign_extend = 0xfffff800 * ((y >> 10) & 1);
                let y = (y | sign_extend) as i32;
                state_snapshot.drawing_offset = (x, y);
                log::info!("drawing offset = {:?}", state_snapshot.drawing_offset,);
            }
            0xe6 => {
                // NOTE: this is also duplicated in the frontend for keeping stat up to date
                // Mask Bit Setting

                //  11    Set mask while drawing (0=TextureBit15, 1=ForceBit15=1)
                //  12    Check mask before draw (0=Draw Always, 1=Draw if Bit15=0)
                let stat_bits_11_12 = (data & 3) << 11;

                gpu_stat
                    .fetch_update(|mut s| {
                        s &= GpuStat::from_bits_retain(!(3 << 11));
                        s |= GpuStat::from_bits_retain(stat_bits_11_12);
                        Some(s)
                    })
                    .unwrap();
            }
            _ => todo!("gp0 environment command {:02X}", cmd),
        }

        None
    }

    fn still_need_params(&mut self) -> bool {
        false
    }

    fn cmd_type(&self) -> Gp0CmdType {
        Gp0CmdType::Environment
    }
}

#[test]
fn cpu_to_vram_interrupt_less_than_1_row() {
    let gpu_stat = Arc::new(AtomicGpuStat::new(GpuStat::default()));
    let mut state_snapshot = GpuStateSnapshot::default();

    let mut cmd = Box::new(CpuToVramBlitCommand::new(0x00000000));
    cmd.add_param(0x00000000); // dest x, y
    cmd.add_param(((10) << 16) | (10)); // size x, y

    assert_eq!(cmd.size, (10, 10));

    // data transfer
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);

    assert!(cmd.still_need_params());

    let backend_cmd = cmd.exec_command(gpu_stat.clone(), &mut state_snapshot);

    if let Some(BackendCommand::WriteVramBlock { block_range, block }) = backend_cmd {
        assert_eq!(block_range, (0..6, 0..1));
        assert_eq!(block, vec![0; 6]);
    } else {
        panic!("expected a WriteVramBlock backend command");
    }
}

#[test]
fn cpu_to_vram_interrupt_more_than_1_row() {
    let gpu_stat = Arc::new(AtomicGpuStat::new(GpuStat::default()));
    let mut state_snapshot = GpuStateSnapshot::default();

    let mut cmd = Box::new(CpuToVramBlitCommand::new(0x00000000));
    cmd.add_param(0x00000000); // dest x, y
    cmd.add_param(((10) << 16) | (10)); // size x, y

    assert_eq!(cmd.size, (10, 10));

    // data transfer
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);

    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);

    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);

    assert!(cmd.still_need_params());

    let backend_cmd = cmd.exec_command(gpu_stat.clone(), &mut state_snapshot);

    // TODO: this is truncating to the full rows, it should also have content of the half rows
    if let Some(BackendCommand::WriteVramBlock { block_range, block }) = backend_cmd {
        assert_eq!(block_range, (0..10, 0..2));
        assert_eq!(block, vec![0; 20]);
    } else {
        panic!("expected a WriteVramBlock backend command");
    }
}

#[test]
fn cpu_to_vram_interrupt_full_rows() {
    let gpu_stat = Arc::new(AtomicGpuStat::new(GpuStat::default()));
    let mut state_snapshot = GpuStateSnapshot::default();

    let mut cmd = Box::new(CpuToVramBlitCommand::new(0x00000000));
    cmd.add_param(0x00000000); // dest x, y
    cmd.add_param(((10) << 16) | (10)); // size x, y

    assert_eq!(cmd.size, (10, 10));

    // data transfer
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);

    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);

    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);
    cmd.add_param(0x00000000);

    assert!(cmd.still_need_params());

    let backend_cmd = cmd.exec_command(gpu_stat.clone(), &mut state_snapshot);

    // TODO: this is truncating to the full rows, it should also have content of the half rows
    if let Some(BackendCommand::WriteVramBlock { block_range, block }) = backend_cmd {
        assert_eq!(block_range, (0..10, 0..3));
        assert_eq!(block, vec![0; 30]);
    } else {
        panic!("expected a WriteVramBlock backend command");
    }
}

#[test]
fn cpu_to_vram_not_interrupted() {
    let gpu_stat = Arc::new(AtomicGpuStat::new(GpuStat::default()));
    let mut state_snapshot = GpuStateSnapshot::default();

    let mut cmd = Box::new(CpuToVramBlitCommand::new(0x00000000));
    cmd.add_param(0x00000000); // dest x, y
    cmd.add_param(((10) << 16) | (10)); // size x, y

    assert_eq!(cmd.size, (10, 10));

    // data transfer
    for _ in 0..5 * 10 {
        cmd.add_param(0);
    }

    assert!(!cmd.still_need_params());

    let backend_cmd = cmd.exec_command(gpu_stat.clone(), &mut state_snapshot);

    // TODO: this is truncating to the full rows, it should also have content of the half rows
    if let Some(BackendCommand::WriteVramBlock { block_range, block }) = backend_cmd {
        assert_eq!(block_range, (0..10, 0..10));
        assert_eq!(block, vec![0; 100]);
    } else {
        panic!("expected a WriteVramBlock backend command");
    }
}
