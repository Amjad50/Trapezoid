use std::env::args;
use std::process::exit;

use psx_core::Psx;

use glium::{glutin, Surface};

fn main() {
    env_logger::builder().format_timestamp(None).init();
    let args: Vec<_> = args().collect();

    if args.len() < 2 {
        println!("USAGE: {} <bios>", args[0]);
        exit(1);
    }

    let event_loop = glutin::event_loop::EventLoop::new();
    let cb = glutin::ContextBuilder::new();
    let wb = glutin::window::WindowBuilder::new();
    let display = glium::Display::new(wb, cb, &event_loop).unwrap();

    let mut psx = Psx::new(&args[1], &display).unwrap();

    loop {
        if psx.clock() {
            let mut frame = display.draw();
            frame.clear_color(0.0, 0.0, 0.0, 0.0);
            psx.blit_to_front(&frame);
            frame.finish().unwrap();
        }
    }
}
