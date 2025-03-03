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
        physical::{PhysicalDevice, PhysicalDeviceType},
        Device, DeviceCreateInfo, DeviceExtensions, Queue, QueueCreateInfo, QueueFlags,
    },
    image::{Image, ImageUsage},
    instance::{Instance, InstanceCreateInfo, InstanceExtensions},
    swapchain::{
        self, CompositeAlpha, PresentMode, Surface, Swapchain, SwapchainCreateInfo,
        SwapchainPresentInfo,
    },
    sync::{self, GpuFuture},
    Validated, VulkanError, VulkanLibrary,
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
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

    fn handle_cpu_state(&mut self, _psx: &mut Psx, _cpu_state: trapezoid_core::cpu::CpuState) {}
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

struct RenderContext {
    window: Arc<Window>,
    swapchain: Arc<Swapchain>,
    images: Vec<Arc<Image>>,
    recreate_swapchain: bool,
}

enum DisplayType {
    Windowed {
        future: Option<Box<dyn GpuFuture>>,
        full_vram_display: bool,
        render_context: Option<RenderContext>,
        event_loop: Option<EventLoop<()>>, // will be taken during `run`
    },
    Headless,
}
impl DisplayType {
    fn is_supported(&self, p: &PhysicalDevice, i: u32) -> bool {
        match self {
            DisplayType::Windowed {
                event_loop: Some(event_loop),
                ..
            } => p.presentation_support(i, &event_loop).unwrap_or(false),
            DisplayType::Windowed {
                event_loop: None, ..
            } => panic!("Event loop is not set"),
            DisplayType::Headless => true, // anything is supported
        }
    }
}

// Locked FPS for audio (more important than video)
// 60 FPS result in popping sound because of emulation speed of the SPU
const FPS: f64 = 59.5;

struct WinitApp {
    instance: Arc<Instance>,
    device: Arc<Device>,
    queue: Arc<Queue>,
    display_type: DisplayType,
    fps: Fps,
    render_time_average: MovingAverage,

    psx: Psx,
    shell_state_open: bool,
    debugger: Debugger,
    audio_player: Option<AudioPlayer<f32>>,
}

impl WinitApp {
    pub fn new(args: &PsxEmuArgs) -> Self {
        let (display_type, instance_create_info) = if args.headless {
            (
                DisplayType::Headless,
                InstanceCreateInfo {
                    enabled_extensions: InstanceExtensions::empty(),
                    ..Default::default()
                },
            )
        } else {
            let event_loop = EventLoop::new().unwrap();
            let required_extensions = Surface::required_extensions(&event_loop).unwrap();
            (
                DisplayType::Windowed {
                    render_context: None,
                    future: None,
                    full_vram_display: args.vram,
                    event_loop: Some(event_loop),
                },
                InstanceCreateInfo {
                    enabled_extensions: required_extensions,
                    ..Default::default()
                },
            )
        };

        let vulkan_library = VulkanLibrary::new().unwrap();
        let instance = Instance::new(vulkan_library, instance_create_info).unwrap();

        let device_extensions = DeviceExtensions {
            khr_swapchain: !args.headless, // only enable swapchain if we have a window
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
                            && display_type.is_supported(&p, i as u32)
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

        let psx = Psx::new(
            &args.bios,
            args.disk_file.clone(),
            PsxConfig {
                stdout_debug: args.debug,
                fast_boot: args.fast_boot,
            },
            device.clone(),
            queue.clone(),
        )
        .unwrap();
        let audio_player = if args.audio {
            let audio_player = AudioPlayer::<f32>::new(44100, BufferSize::QuarterSecond);

            match audio_player {
                Ok(p) => {
                    p.play().expect("Audio device to play");
                    Some(p)
                }
                Err(e) => {
                    log::error!("Failed to initialize audio player: {:?}", e);
                    None
                }
            }
        } else {
            None
        };

        Self {
            instance,
            device: device.clone(),
            queue: queue.clone(),
            display_type,
            fps: Fps::new(FPS),
            render_time_average: MovingAverage::new(),

            psx,

            shell_state_open: false,
            debugger: Debugger::new(),
            audio_player,
        }
    }

    fn render_frame(&mut self) {
        match &mut self.display_type {
            DisplayType::Windowed {
                render_context: None,
                ..
            } => {
                log::error!("No render context");
            }
            DisplayType::Windowed {
                full_vram_display,
                future,
                render_context:
                    Some(RenderContext {
                        swapchain,
                        images,
                        recreate_swapchain,
                        window,
                        ..
                    }),
                ..
            } => {
                let t = Instant::now();
                let mut current_future = future.take().unwrap();
                current_future.cleanup_finished();

                window.set_title(&format!(
                    "PSX - FPS: {:.1} - Render time: {:.1}us",
                    (self.fps.fps() * 10.).round() / 10.,
                    (self.render_time_average.average() * 10.).round() / 10.
                ));

                if *recreate_swapchain {
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

                let (image_num, suboptimal, acquire_future) =
                    match swapchain::acquire_next_image(swapchain.clone(), None)
                        .map_err(Validated::unwrap)
                    {
                        Ok(r) => r,
                        Err(VulkanError::OutOfDate) => {
                            *recreate_swapchain = true;
                            return;
                        }
                        Err(e) => panic!("Failed to acquire next image: {:?}", e),
                    };

                if suboptimal {
                    *recreate_swapchain = true;
                }

                let current_image = images[image_num as usize].clone();

                let current_future = self.psx.blit_to_front(
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

    fn handle_redraw_requested(&mut self) {
        // limit the frame rate to the target fps if the display support more than that
        self.fps.lock();
        self.fps.tick();

        // if the debugger is enabled, we don't run the emulation
        if !self.debugger.enabled() {
            let cpu_state = self.psx.clock_full_video_frame();
            self.debugger.handle_cpu_state(&mut self.psx, cpu_state);

            let audio_buffer = self.psx.take_audio_buffer();
            if let Some(audio_player) = &mut self.audio_player {
                audio_player.queue(&audio_buffer);
            }
        }
        // keep rendering even when debugger is running so that
        // we don't hang the display
        self.render_frame();
    }

    fn run(&mut self) {
        match &mut self.display_type {
            DisplayType::Windowed { event_loop, .. } => {
                let event_loop = event_loop.take().unwrap();
                event_loop.run_app(self).unwrap();
            }
            DisplayType::Headless => loop {
                self.handle_redraw_requested();
                std::thread::sleep(Duration::from_millis(1));
            },
        }
    }
}

impl ApplicationHandler for WinitApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        match &mut self.display_type {
            DisplayType::Windowed {
                render_context,
                future,
                ..
            } => {
                let window = Arc::new(
                    event_loop
                        .create_window(Window::default_attributes())
                        .unwrap(),
                );

                let surface = Surface::from_window(self.instance.clone(), window.clone()).unwrap();

                let (swapchain, images) = {
                    let caps = self
                        .device
                        .physical_device()
                        .surface_capabilities(&surface, Default::default())
                        .unwrap();

                    let format = self
                        .device
                        .physical_device()
                        .surface_formats(&surface, Default::default())
                        .unwrap()[0]
                        .0;
                    let window = surface.object().unwrap().downcast_ref::<Window>().unwrap();

                    let present_mode = self
                        .device
                        .physical_device()
                        .surface_present_modes(&surface, Default::default())
                        .unwrap()
                        .into_iter()
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
                        self.device.clone(),
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

                assert!(render_context.is_none());
                *render_context = Some(RenderContext {
                    window,
                    swapchain,
                    images,
                    recreate_swapchain: false,
                });
                *future = Some(sync::now(self.device.clone()).boxed());
            }
            DisplayType::Headless => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let DisplayType::Windowed {
            render_context: Some(RenderContext { window, .. }),
            ..
        } = &mut self.display_type
        {
            window.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(_) => {
                if let DisplayType::Windowed {
                    render_context:
                        Some(RenderContext {
                            ref mut recreate_swapchain,
                            ..
                        }),
                    ..
                } = &mut self.display_type
                {
                    *recreate_swapchain = true;
                }
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
                    self.psx.change_controller_key_state(k, pressed);
                } else if pressed {
                    match input.physical_key {
                        #[cfg(feature = "debugger")]
                        // Pause CPU and enable debug
                        PhysicalKey::Code(KeyCode::Slash) => {
                            println!("{:?}", self.psx.cpu().registers());
                            self.debugger.set_enabled(true);
                        }
                        #[cfg(feature = "debugger")]
                        // Resume CPU if paused
                        PhysicalKey::Code(KeyCode::KeyC) => {
                            self.debugger.set_enabled(false);
                        }
                        PhysicalKey::Code(KeyCode::KeyV) => self.toggle_full_vram_display(),
                        PhysicalKey::Code(KeyCode::BracketRight) => {
                            self.shell_state_open = !self.shell_state_open;
                            self.psx
                                .change_cdrom_shell_open_state(self.shell_state_open);
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.handle_redraw_requested();
            }
            _ => {}
        }
        // this is placed outside the emulation event, so that it reacts faster
        // to user input
        if self.debugger.enabled() {
            self.debugger.run(&mut self.psx);
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

    let mut app = WinitApp::new(&args);
    app.run();
}
