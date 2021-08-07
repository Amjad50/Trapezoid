use std::path::PathBuf;
use std::rc::Rc;

use psx_core::{DigitalControllerKey, Psx};

use clap::Clap;
use glium::{
    glutin::{
        self,
        event::{ElementState, Event, VirtualKeyCode, WindowEvent},
        event_loop::ControlFlow,
    },
    Surface,
};

enum GlDisplay {
    Headless(glium::HeadlessRenderer),
    Windowed(glium::Display),
}

impl GlDisplay {
    fn windowed(event_loop: &glutin::event_loop::EventLoop<()>) -> Self {
        let cb = glutin::ContextBuilder::new();
        let wb = glutin::window::WindowBuilder::new();

        Self::Windowed(glium::Display::new(wb, cb, &event_loop).unwrap())
    }

    fn headless(event_loop: &glutin::event_loop::EventLoop<()>, width: u32, height: u32) -> Self {
        let cb = glutin::ContextBuilder::new();
        let context = cb
            .build_headless(event_loop, glutin::dpi::PhysicalSize::new(width, height))
            .unwrap();

        Self::Headless(glium::HeadlessRenderer::new(context).unwrap())
    }

    fn render_frame(&self, psx: &Psx) {
        match self {
            GlDisplay::Windowed(display) => {
                let mut frame = display.draw();
                frame.clear_color(0.0, 0.0, 0.0, 0.0);
                psx.blit_to_front(&frame);
                frame.finish().unwrap();
            }
            GlDisplay::Headless(_) => {}
        }
    }
}

impl glium::backend::Facade for GlDisplay {
    fn get_context(&self) -> &Rc<glium::backend::Context> {
        match self {
            GlDisplay::Headless(headless) => headless.get_context(),
            GlDisplay::Windowed(display) => display.get_context(),
        }
    }
}

#[derive(Clap, Debug)]
#[clap(version = "0.1.0", author = "Amjad Alsharafi", about = "PSX emulator")]
struct PsxEmuArgs {
    bios: PathBuf,
    disk_file: Option<PathBuf>,
    #[clap(short, long)]
    windowed: bool,
}

fn main() {
    env_logger::builder().format_timestamp(None).init();

    let args = PsxEmuArgs::parse();

    let event_loop = glutin::event_loop::EventLoop::new();
    let display = if args.windowed {
        GlDisplay::windowed(&event_loop)
    } else {
        GlDisplay::headless(&event_loop, 800, 600)
    };

    let mut psx = Psx::new(&args.bios, args.disk_file, &display).unwrap();

    event_loop.run(move |event, _target, control_flow| {
        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                WindowEvent::KeyboardInput { input, .. } => {
                    // TODO: handle all keys, this is just for testing temporary
                    if let Some(VirtualKeyCode::Return) = input.virtual_keycode {
                        let pressed = input.state == ElementState::Pressed;

                        psx.change_controller_key_state(DigitalControllerKey::Start, pressed);
                    }
                }
                _ => {}
            },
            _ => {}
        }
        *control_flow = ControlFlow::Poll;

        // do several clocks in one time to reduce latency of the `event_loop.run` method.
        for _ in 0..100 {
            if psx.clock() {
                display.render_frame(&psx);
            }
        }
    });
}
