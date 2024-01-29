#[cfg(feature = "debugger")]
mod debugger;

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use dynwave::{AudioPlayer, BufferSize};
use trapezoid_core::{DigitalControllerKey, Psx, PsxConfig};

use clap::Parser;
use vulkano::{
    device::{
        physical::PhysicalDeviceType, Device, DeviceCreateInfo, DeviceExtensions, Queue,
        QueueCreateInfo, QueueFlags,
    },
    image::{Image, ImageUsage},
    instance::{Instance, InstanceCreateFlags, InstanceCreateInfo, InstanceExtensions},
    swapchain::{
        self, CompositeAlpha, PresentMode, Surface, Swapchain, SwapchainCreateInfo,
        SwapchainPresentInfo,
    },
    sync::{self, GpuFuture},
    Validated, VulkanError, VulkanLibrary,
};
use winit::{
    event::{ElementState, Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowBuilder, WindowId},
};

#[cfg(feature = "debugger")]
use debugger::Debugger;

#[cfg(not(feature = "debugger"))]
struct Debugger;

#[cfg(not(feature = "debugger"))]
impl Debugger {
    fn new() -> Self {
        Self {}
    }

    fn enabled(&self) -> bool {
        false
    }

    fn run(&mut self, _psx: &mut Psx) {}

    fn handle_cpu_state(&mut self, _psx: &mut Psx, _cpu_state: psx_core::cpu::CpuState) {}
}

struct MovingAverage {
    values: [f64; 100],
    current_index: usize,
    sum: f64,
}

impl MovingAverage {
    fn new() -> Self {
        Self {
            values: [0.0; 100],
            current_index: 0,
            sum: 0.0,
        }
    }

    fn add(&mut self, value: f64) {
        self.sum -= self.values[self.current_index];
        self.sum += value;
        self.values[self.current_index] = value;
        self.current_index = (self.current_index + 1) % self.values.len();
    }

    fn average(&self) -> f64 {
        self.sum / self.values.len() as f64
    }
}

/// Moving average fps counter
struct Fps {
    moving_average: MovingAverage,
    last_frame: Instant,
    target_fps: f64,
}

impl Fps {
    fn new(target_fps: f64) -> Self {
        Self {
            moving_average: MovingAverage::new(),
            last_frame: Instant::now(),
            target_fps,
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        let delta = now.duration_since(self.last_frame).as_secs_f64();
        self.last_frame = now;

        self.moving_average.add(delta);
    }

    fn fps(&self) -> f64 {
        1.0 / self.moving_average.average()
    }

    /// Locks the current thread to the target FPS
    /// This is useful when running on a higher FPS than 60
    fn lock(&mut self) {
        let duration_per_frame = Duration::from_secs_f64(1.0 / self.target_fps);

        let elapsed = self.last_frame.elapsed();

        if elapsed >= duration_per_frame {
            return;
        }

        let remaining = duration_per_frame - elapsed;
        if remaining > Duration::from_millis(1) {
            std::thread::sleep(remaining - Duration::from_millis(1));
            let elapsed = self.last_frame.elapsed();
            if elapsed >= duration_per_frame {
                return;
            }
        }
        // spinlock for the remaining time
        while self.last_frame.elapsed() < duration_per_frame {
            std::hint::spin_loop();
        }
    }
}

enum DisplayType {
    Windowed {
        event_loop: Option<EventLoop<()>>,
        window: Arc<Window>,
        surface: Arc<Surface>,
        swapchain: Arc<Swapchain>,
        images: Vec<Arc<Image>>,
        future: Option<Box<dyn GpuFuture>>,
        full_vram_display: bool,
    },
    Headless,
}

// Locked FPS for audio (more important than video)
// 60 FPS result in popping sound because of emulation speed of the SPU
const FPS: f64 = 59.5;

struct VkDisplay {
    device: Arc<Device>,
    queue: Arc<Queue>,
    display_type: DisplayType,
    fps: Fps,
    render_time_average: MovingAverage,
}

impl VkDisplay {
    fn windowed(full_vram_display: bool) -> Self {
        let event_loop = EventLoop::new().unwrap();

        let vulkan_library = VulkanLibrary::new().unwrap();
        let required_extensions = Surface::required_extensions(&event_loop);

        let instance = Instance::new(
            vulkan_library,
            InstanceCreateInfo {
                flags: InstanceCreateFlags::ENUMERATE_PORTABILITY,
                enabled_extensions: required_extensions,
                ..Default::default()
            },
        )
        .unwrap();

        let window = Arc::new(WindowBuilder::new().build(&event_loop).unwrap());
        let surface = Surface::from_window(instance.clone(), window.clone()).unwrap();

        let device_extensions = DeviceExtensions {
            khr_swapchain: true,
            ..DeviceExtensions::empty()
        };

        let (physical_device, queue_family_index) = instance
            .enumerate_physical_devices()
            .unwrap()
            .filter(|p| p.supported_extensions().contains(&device_extensions))
            .filter_map(|p| {
                p.queue_family_properties()
                    .iter()
                    .enumerate()
                    .position(|(i, q)| {
                        q.queue_flags
                            .contains(QueueFlags::GRAPHICS | QueueFlags::COMPUTE)
                            && p.surface_support(i as u32, &surface).unwrap_or(false)
                    })
                    .map(|i| (p, i as u32))
            })
            .min_by_key(|(p, _)| match p.properties().device_type {
                PhysicalDeviceType::DiscreteGpu => 0,
                PhysicalDeviceType::IntegratedGpu => 1,
                PhysicalDeviceType::VirtualGpu => 2,
                PhysicalDeviceType::Cpu => 3,
                PhysicalDeviceType::Other => 4,
                _ => 5,
            })
            .unwrap();

        println!(
            "Using device: {} (type: {:?})",
            physical_device.properties().device_name,
            physical_device.properties().device_type,
        );

        let (device, mut queues) = Device::new(
            physical_device,
            DeviceCreateInfo {
                enabled_extensions: device_extensions,
                queue_create_infos: vec![QueueCreateInfo {
                    queue_family_index,
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .unwrap();

        let queue = queues.next().unwrap();

        let (swapchain, images) = {
            let caps = device
                .physical_device()
                .surface_capabilities(&surface, Default::default())
                .unwrap();

            let format = device
                .physical_device()
                .surface_formats(&surface, Default::default())
                .unwrap()[0]
                .0;
            let window = surface.object().unwrap().downcast_ref::<Window>().unwrap();

            let present_mode = device
                .physical_device()
                .surface_present_modes(&surface, Default::default())
                .unwrap()
                .min_by_key(|&m| match m {
                    PresentMode::Mailbox => 0,
                    PresentMode::Immediate => 1,
                    PresentMode::Fifo => 2,
                    PresentMode::FifoRelaxed => 3,
                    _ => 4,
                })
                .unwrap();

            let dimensions: [u32; 2] = window.inner_size().into();
            Swapchain::new(
                device.clone(),
                surface.clone(),
                SwapchainCreateInfo {
                    min_image_count: caps.min_image_count,
                    image_format: format,
                    image_extent: dimensions,
                    image_usage: ImageUsage::TRANSFER_DST,
                    composite_alpha: CompositeAlpha::Opaque,
                    present_mode,
                    ..Default::default()
                },
            )
            .unwrap()
        };

        Self {
            device: device.clone(),
            queue,
            fps: Fps::new(FPS),
            render_time_average: MovingAverage::new(),
            display_type: DisplayType::Windowed {
                event_loop: Some(event_loop),
                window,
                surface,
                swapchain,
                images,
                full_vram_display,
                future: Some(sync::now(device).boxed()),
            },
        }
    }

    fn headless() -> Self {
        let vulkan_library = VulkanLibrary::new().unwrap();

        let instance = Instance::new(
            vulkan_library,
            InstanceCreateInfo {
                enabled_extensions: InstanceExtensions::empty(),
                ..Default::default()
            },
        )
        .unwrap();

        let (physical_device, queue_family_index) = instance
            .enumerate_physical_devices()
            .unwrap()
            .filter_map(|p| {
                p.queue_family_properties()
                    .iter()
                    .position(|q| {
                        q.queue_flags
                            .contains(QueueFlags::GRAPHICS | QueueFlags::COMPUTE)
                    })
                    .map(|i| (p, i as u32))
            })
            .min_by_key(|(p, _)| match p.properties().device_type {
                PhysicalDeviceType::DiscreteGpu => 0,
                PhysicalDeviceType::IntegratedGpu => 1,
                PhysicalDeviceType::VirtualGpu => 2,
                PhysicalDeviceType::Cpu => 3,
                PhysicalDeviceType::Other => 4,
                _ => 5,
            })
            .unwrap();

        println!(
            "Using device: {} (type: {:?})",
            physical_device.properties().device_name,
            physical_device.properties().device_type,
        );

        let (device, mut queues) = Device::new(
            physical_device,
            DeviceCreateInfo {
                queue_create_infos: vec![QueueCreateInfo {
                    queue_family_index,
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .unwrap();

        let queue = queues.next().unwrap();

        Self {
            device,
            queue,
            fps: Fps::new(FPS),
            render_time_average: MovingAverage::new(),
            display_type: DisplayType::Headless,
        }
    }

    fn window_resize(&mut self) {
        match &mut self.display_type {
            DisplayType::Windowed {
                surface,
                swapchain,
                images,
                ..
            } => {
                let window = surface.object().unwrap().downcast_ref::<Window>().unwrap();
                let dimensions: [u32; 2] = window.inner_size().into();
                let (new_swapchain, new_images) = swapchain
                    .recreate(SwapchainCreateInfo {
                        image_extent: dimensions,
                        ..swapchain.create_info()
                    })
                    .expect("failed to recreate swapchain");

                *swapchain = new_swapchain;
                *images = new_images;
            }
            DisplayType::Headless => {}
        }
    }

    fn render_frame(&mut self, psx: &mut Psx) {
        let mut recreate_swapchain = false;
        match &mut self.display_type {
            DisplayType::Windowed {
                swapchain,
                images,
                full_vram_display,
                surface,
                future,
                ..
            } => {
                let t = Instant::now();
                let mut current_future = future.take().unwrap();
                current_future.cleanup_finished();

                let window = surface.object().unwrap().downcast_ref::<Window>().unwrap();
                window.set_title(&format!(
                    "PSX - FPS: {:.1} - Render time: {:.1}us",
                    (self.fps.fps() * 10.).round() / 10.,
                    (self.render_time_average.average() * 10.).round() / 10.
                ));

                let (image_num, suboptimal, acquire_future) =
                    match swapchain::acquire_next_image(swapchain.clone(), None)
                        .map_err(Validated::unwrap)
                    {
                        Ok(r) => r,
                        Err(VulkanError::OutOfDate) => {
                            panic!("recreate swapchain");
                        }
                        Err(e) => panic!("Failed to acquire next image: {:?}", e),
                    };

                if suboptimal {
                    recreate_swapchain = true;
                }

                let current_image = images[image_num as usize].clone();

                let current_future = psx.blit_to_front(
                    current_image,
                    *full_vram_display,
                    current_future.join(acquire_future).boxed(),
                );

                *future = Some(
                    current_future
                        .then_swapchain_present(
                            self.queue.clone(),
                            SwapchainPresentInfo::swapchain_image_index(
                                swapchain.clone(),
                                image_num,
                            ),
                        )
                        .then_signal_fence_and_flush()
                        .unwrap()
                        .boxed(),
                );

                let elapsed = t.elapsed();
                self.render_time_average.add(elapsed.as_micros() as f64);
            }
            DisplayType::Headless => {}
        }

        if recreate_swapchain {
            // handles swapchain recreation
            self.window_resize();
        }
    }

    fn toggle_full_vram_display(&mut self) {
        match self.display_type {
            DisplayType::Windowed {
                ref mut full_vram_display,
                ..
            } => {
                *full_vram_display = !*full_vram_display;
            }
            DisplayType::Headless => {}
        }
    }

    fn run<F>(mut self, mut f: F)
    where
        F: 'static + FnMut(&mut VkDisplay, Event<()>) -> Option<ControlFlow>,
    {
        match self.display_type {
            DisplayType::Windowed {
                ref mut event_loop,
                ref window,
                ..
            } => {
                let event_loop = event_loop.take().unwrap();
                let window = window.clone();
                event_loop
                    .run(|event, target| match event {
                        Event::AboutToWait => {
                            window.request_redraw();
                        }
                        _ => {
                            let r = f(&mut self, event);
                            if let Some(r) = r {
                                target.set_control_flow(r);
                            } else {
                                target.exit();
                            }
                        }
                    })
                    .unwrap();
            }
            DisplayType::Headless => loop {
                // TODO: support keyboard input and such
                // NOTE: MainEventCleared is used here to run the emulator
                let r = f(
                    &mut self,
                    Event::WindowEvent {
                        window_id: unsafe { WindowId::dummy() },
                        event: WindowEvent::RedrawRequested,
                    },
                );
                if r.is_none() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            },
        }
    }
}

#[derive(Parser, Debug)]
#[command(version, author, about = "PSX emulator")]
struct PsxEmuArgs {
    /// The bios file to run
    bios: PathBuf,
    /// The disk/exe file to run, without this, it will run the bios only
    disk_file: Option<PathBuf>,
    /// Turn off window display and run in headless mode
    #[arg(short = 'e', long)]
    headless: bool,
    /// Initial value for `display full vram`, can be changed later with [V] key
    #[arg(short, long)]
    vram: bool,
    /// Play audio
    #[arg(short, long)]
    audio: bool,
    /// Print tty debug output to the console
    #[arg(short, long)]
    debug: bool,
    /// Skips the shell
    #[arg(short, long)]
    fast_boot: bool,
}

fn main() {
    env_logger::builder()
        .format_timestamp(None)
        .filter_level(log::LevelFilter::Error)
        .init();

    let args = PsxEmuArgs::parse();

    let display = if args.headless {
        VkDisplay::headless()
    } else {
        VkDisplay::windowed(args.vram)
    };

    let mut psx = Psx::new(
        &args.bios,
        args.disk_file,
        PsxConfig {
            stdout_debug: args.debug,
            fast_boot: args.fast_boot,
        },
        display.device.clone(),
        display.queue.clone(),
    )
    .unwrap();

    let mut shell_state_open = false;

    let mut debugger = Debugger::new();

    let mut audio_player = AudioPlayer::<f32>::new(44100, BufferSize::QuarterSecond).unwrap();
    if args.audio {
        audio_player.play().unwrap();
    }

    display.run(move |display, event| {
        if let Event::WindowEvent { event, .. } = event {
            match event {
                WindowEvent::CloseRequested => {
                    return None;
                }
                WindowEvent::Resized(_) => {
                    display.window_resize();
                }
                WindowEvent::KeyboardInput { event: input, .. } => {
                    let pressed = input.state == ElementState::Pressed;

                    let digital_key = match input.physical_key {
                        PhysicalKey::Code(KeyCode::Enter) => Some(DigitalControllerKey::Start),
                        PhysicalKey::Code(KeyCode::Backspace) => Some(DigitalControllerKey::Select),

                        PhysicalKey::Code(KeyCode::Digit1) => Some(DigitalControllerKey::L1),
                        PhysicalKey::Code(KeyCode::Digit2) => Some(DigitalControllerKey::L2),
                        PhysicalKey::Code(KeyCode::Digit3) => Some(DigitalControllerKey::L3),
                        PhysicalKey::Code(KeyCode::Digit0) => Some(DigitalControllerKey::R1),
                        PhysicalKey::Code(KeyCode::Digit9) => Some(DigitalControllerKey::R2),
                        PhysicalKey::Code(KeyCode::Digit8) => Some(DigitalControllerKey::R3),

                        PhysicalKey::Code(KeyCode::KeyW) => Some(DigitalControllerKey::Up),
                        PhysicalKey::Code(KeyCode::KeyS) => Some(DigitalControllerKey::Down),
                        PhysicalKey::Code(KeyCode::KeyD) => Some(DigitalControllerKey::Right),
                        PhysicalKey::Code(KeyCode::KeyA) => Some(DigitalControllerKey::Left),

                        PhysicalKey::Code(KeyCode::KeyI) => Some(DigitalControllerKey::Triangle),
                        PhysicalKey::Code(KeyCode::KeyK) => Some(DigitalControllerKey::X),
                        PhysicalKey::Code(KeyCode::KeyL) => Some(DigitalControllerKey::Circle),
                        PhysicalKey::Code(KeyCode::KeyJ) => Some(DigitalControllerKey::Square),
                        _ => None,
                    };
                    if let Some(k) = digital_key {
                        psx.change_controller_key_state(k, pressed);
                    } else if pressed {
                        match input.physical_key {
                            #[cfg(feature = "debugger")]
                            // Pause CPU and enable debug
                            PhysicalKey::Code(KeyCode::Slash) => {
                                println!("{:?}", psx.cpu().registers());
                                debugger.set_enabled(true);
                            }
                            PhysicalKey::Code(KeyCode::KeyV) => display.toggle_full_vram_display(),
                            PhysicalKey::Code(KeyCode::BracketRight) => {
                                shell_state_open = !shell_state_open;
                                psx.change_cdrom_shell_open_state(shell_state_open);
                            }
                            _ => {}
                        }
                    }
                }
                WindowEvent::RedrawRequested => {
                    // limit the frame rate to the target fps if the display support more than that
                    display.fps.lock();
                    display.fps.tick();

                    // if the debugger is enabled, we don't run the emulation
                    if !debugger.enabled() {
                        let cpu_state = psx.clock_full_video_frame();
                        debugger.handle_cpu_state(&mut psx, cpu_state);

                        let audio_buffer = psx.take_audio_buffer();
                        if args.audio {
                            audio_player.queue(&audio_buffer);
                        }
                    }
                    // keep rendering even when debugger is  running so that
                    // we don't hang the display
                    display.render_frame(&mut psx);
                }
                _ => {}
            }
        }
        // this is placed outside the emulation event, so that it reacts faster
        // to user input
        if debugger.enabled() {
            debugger.run(&mut psx);
        }

        Some(ControlFlow::Poll)
    });
}
