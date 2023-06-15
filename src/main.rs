mod audio;
#[cfg(feature = "debugger")]
mod debugger;

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use audio::AudioPlayer;
use psx_core::{DigitalControllerKey, Psx, PsxConfig};

use clap::Parser;
use vulkano::{
    device::{
        physical::PhysicalDeviceType, Device, DeviceCreateInfo, DeviceExtensions, Queue,
        QueueCreateInfo, QueueFlags,
    },
    image::{ImageUsage, SwapchainImage},
    instance::{Instance, InstanceCreateInfo, InstanceExtensions},
    swapchain::{
        self, AcquireError, CompositeAlpha, PresentMode, Surface, Swapchain, SwapchainCreateInfo,
        SwapchainCreationError, SwapchainPresentInfo,
    },
    sync::{self, GpuFuture},
    VulkanLibrary,
};
use vulkano_win::VkSurfaceBuild;
use winit::{
    event::{ElementState, Event, VirtualKeyCode, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::{Window, WindowBuilder},
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

/// Moving average fps counter
struct FPS {
    frames: [f64; 100],
    current_index: usize,
    sum: f64,
    last_frame: Instant,
    last_instance_check: Instant,
    check_offset: Duration,
}

impl FPS {
    fn new() -> Self {
        Self {
            frames: [0.0; 100],
            current_index: 0,
            sum: 0.0,
            last_frame: Instant::now(),
            last_instance_check: Instant::now(),
            check_offset: Duration::from_millis(0),
        }
    }

    fn tick(&mut self) -> f64 {
        let now = Instant::now();
        let delta = now.duration_since(self.last_frame).as_secs_f64();
        self.last_frame = now;

        self.sum -= self.frames[self.current_index];
        self.sum += delta;
        self.frames[self.current_index] = delta;
        self.current_index = (self.current_index + 1) % self.frames.len();

        self.frames.len() as f64 / self.sum
    }

    fn fps(&self) -> f64 {
        self.frames.len() as f64 / self.sum
    }

    fn did_reach_target(&mut self, target_fps: u64) -> bool {
        let duration_per_frame = Duration::from_micros(1_000_000 / target_fps);

        let elapsed = self.last_frame.elapsed().saturating_sub(self.check_offset);

        // this gives us the approx time since the last check
        // for high refresh rates, this will be smaller than the expected 60 FPS
        let elapsed_since_last_check = self.last_instance_check.elapsed();
        self.last_instance_check = Instant::now();

        if elapsed >= duration_per_frame {
            true
        } else {
            let remaining = duration_per_frame - elapsed;
            // if we will reach in the middle, then allow this time, but add an offset for next frame
            if elapsed_since_last_check >= remaining {
                // we have offsetted the check, so it affects the next frame
                self.check_offset = elapsed_since_last_check - remaining;
                true
            } else {
                false
            }
        }
    }
}

enum DisplayType {
    Windowed {
        event_loop: Option<EventLoop<()>>,
        surface: Arc<Surface>,
        swapchain: Arc<Swapchain>,
        images: Vec<Arc<SwapchainImage>>,
        future: Option<Box<dyn GpuFuture>>,
        full_vram_display: bool,
    },
    Headless,
}

struct VkDisplay {
    device: Arc<Device>,
    queue: Arc<Queue>,
    display_type: DisplayType,
    fps: FPS,
}

impl VkDisplay {
    fn windowed(full_vram_display: bool) -> Self {
        let event_loop = EventLoop::new();

        let vulkan_library = VulkanLibrary::new().unwrap();
        let required_extensions = vulkano_win::required_extensions(&vulkan_library);

        let instance = Instance::new(
            vulkan_library,
            InstanceCreateInfo {
                enabled_extensions: required_extensions,
                ..Default::default()
            },
        )
        .unwrap();

        let surface = WindowBuilder::new()
            .build_vk_surface(&event_loop, instance.clone())
            .unwrap();

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

            let format = Some(
                device
                    .physical_device()
                    .surface_formats(&surface, Default::default())
                    .unwrap()[0]
                    .0,
            );
            let window = surface.object().unwrap().downcast_ref::<Window>().unwrap();

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
                    present_mode: PresentMode::Fifo,
                    ..Default::default()
                },
            )
            .unwrap()
        };

        Self {
            device: device.clone(),
            queue,
            fps: FPS::new(),
            display_type: DisplayType::Windowed {
                event_loop: Some(event_loop),
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
            fps: FPS::new(),
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
                let (new_swapchain, new_images) = match swapchain.recreate(SwapchainCreateInfo {
                    image_extent: dimensions,
                    ..swapchain.create_info()
                }) {
                    Ok(r) => r,
                    Err(SwapchainCreationError::ImageExtentNotSupported { .. }) => return,
                    Err(e) => panic!("Failed to recreate swapchain: {:?}", e),
                };

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
                let mut current_future = future.take().unwrap();
                current_future.cleanup_finished();

                let window = surface.object().unwrap().downcast_ref::<Window>().unwrap();
                window.set_title(&format!(
                    "PSX - FPS: {:.1}",
                    (self.fps.fps() * 10.).round() / 10.
                ));

                let (image_num, suboptimal, acquire_future) =
                    match swapchain::acquire_next_image(swapchain.clone(), None) {
                        Ok(r) => r,
                        Err(AcquireError::OutOfDate) => {
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
        F: 'static + FnMut(&mut VkDisplay, Event<'_, ()>) -> ControlFlow,
    {
        match self.display_type {
            DisplayType::Windowed {
                ref mut event_loop, ..
            } => {
                let event_loop = event_loop.take().unwrap();
                event_loop.run(move |event, _target, control_flow| {
                    *control_flow = f(&mut self, event);
                });
            }
            DisplayType::Headless => loop {
                // TODO: support keyboard input and such
                // NOTE: MainEventCleared is used here to run the emulator
                let _ = f(&mut self, Event::MainEventsCleared);
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
    env_logger::builder().format_timestamp(None).init();

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

    let mut debugger = Debugger::new();

    let mut audio_player = AudioPlayer::new(44100);
    if args.audio {
        audio_player.play();
    }

    display.run(move |display, event| {
        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    return ControlFlow::Exit;
                }
                WindowEvent::Resized(_) => {
                    display.window_resize();
                }
                WindowEvent::KeyboardInput { input, .. } => {
                    let pressed = input.state == ElementState::Pressed;

                    let digital_key = match input.virtual_keycode {
                        Some(VirtualKeyCode::Return) => Some(DigitalControllerKey::Start),
                        Some(VirtualKeyCode::Back) => Some(DigitalControllerKey::Select),

                        Some(VirtualKeyCode::Key1) => Some(DigitalControllerKey::L1),
                        Some(VirtualKeyCode::Key2) => Some(DigitalControllerKey::L2),
                        Some(VirtualKeyCode::Key3) => Some(DigitalControllerKey::L3),
                        Some(VirtualKeyCode::Key0) => Some(DigitalControllerKey::R1),
                        Some(VirtualKeyCode::Key9) => Some(DigitalControllerKey::R2),
                        Some(VirtualKeyCode::Key8) => Some(DigitalControllerKey::R3),

                        Some(VirtualKeyCode::W) => Some(DigitalControllerKey::Up),
                        Some(VirtualKeyCode::S) => Some(DigitalControllerKey::Down),
                        Some(VirtualKeyCode::D) => Some(DigitalControllerKey::Right),
                        Some(VirtualKeyCode::A) => Some(DigitalControllerKey::Left),

                        Some(VirtualKeyCode::I) => Some(DigitalControllerKey::Triangle),
                        Some(VirtualKeyCode::K) => Some(DigitalControllerKey::X),
                        Some(VirtualKeyCode::L) => Some(DigitalControllerKey::Circle),
                        Some(VirtualKeyCode::J) => Some(DigitalControllerKey::Square),

                        _ => None,
                    };
                    if let Some(k) = digital_key {
                        psx.change_controller_key_state(k, pressed);
                    } else if pressed {
                        match input.virtual_keycode {
                            #[cfg(feature = "debugger")]
                            // Pause CPU and enable debug
                            Some(VirtualKeyCode::Slash) => {
                                println!("{:?}", psx.cpu().registers());
                                debugger.set_enabled(true);
                            }
                            Some(VirtualKeyCode::V) => display.toggle_full_vram_display(),
                            _ => {}
                        }
                    }
                }
                _ => {}
            },
            Event::MainEventsCleared => {
                // limit the frame rate to 60 fps if the display support more than that
                if !display.fps.did_reach_target(60) {
                    return ControlFlow::Poll;
                }
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

        // this is placed outside the emulation event, so that it reacts faster
        // to user input
        if debugger.enabled() {
            debugger.run(&mut psx);
        }

        ControlFlow::Poll
    });
}
