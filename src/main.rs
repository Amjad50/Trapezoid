mod audio;

use std::{path::PathBuf, sync::Arc, time::Instant};

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

enum DisplayType {
    Windowed {
        surface: Arc<Surface>,
        swapchain: Arc<Swapchain>,
        images: Vec<Arc<SwapchainImage>>,
        future: Option<Box<dyn GpuFuture>>,
        full_vram_display: bool,
        last_frame_time: Instant,
    },
    Headless,
}

struct VkDisplay {
    device: Arc<Device>,
    queue: Arc<Queue>,
    display_type: DisplayType,
}

impl VkDisplay {
    fn windowed(event_loop: &EventLoop<()>, full_vram_display: bool) -> Self {
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
            .build_vk_surface(event_loop, instance.clone())
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
                        q.queue_flags.intersects(&QueueFlags {
                            graphics: true,
                            compute: true,
                            ..QueueFlags::empty()
                        }) && p.surface_support(i as u32, &surface).unwrap_or(false)
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
                    image_usage: ImageUsage {
                        transfer_dst: true,
                        ..ImageUsage::empty()
                    },
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
            display_type: DisplayType::Windowed {
                surface,
                swapchain,
                images,
                full_vram_display,
                future: Some(sync::now(device).boxed()),
                last_frame_time: Instant::now(),
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
                        q.queue_flags.intersects(&QueueFlags {
                            graphics: true,
                            ..QueueFlags::empty()
                        })
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
        match &mut self.display_type {
            DisplayType::Windowed {
                swapchain,
                images,
                full_vram_display,
                surface,
                future,
                last_frame_time,
            } => {
                let mut current_future = future.take().unwrap();
                current_future.cleanup_finished();

                let window = surface.object().unwrap().downcast_ref::<Window>().unwrap();
                window.set_title(&format!(
                    "PSX - FPS: {}",
                    (1. / last_frame_time.elapsed().as_secs_f32()).round()
                ));

                // reset timer
                *last_frame_time = Instant::now();

                let (image_num, suboptimal, acquire_future) =
                    match swapchain::acquire_next_image(swapchain.clone(), None) {
                        Ok(r) => r,
                        Err(AcquireError::OutOfDate) => {
                            panic!("recreate swapchain");
                        }
                        Err(e) => panic!("Failed to acquire next image: {:?}", e),
                    };

                if suboptimal {
                    panic!("recreate swapchain");
                    //recreate_swapchain = true;
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
}

#[derive(Parser, Debug)]
#[clap(version = "0.1.0", author = "Amjad Alsharafi", about = "PSX emulator")]
struct PsxEmuArgs {
    /// The bios file to run
    bios: PathBuf,
    /// The disk file to run, without this, it will run the bios only
    disk_file: Option<PathBuf>,
    /// Turn on window display (without this, it will only print the
    /// logs to the console, which can be useful for testing)
    #[clap(short, long)]
    windowed: bool,
    /// Initial value for `display full vram`, can be changed later with [V] key
    #[clap(short, long)]
    vram: bool,
    /// Play audio
    #[clap(short, long)]
    audio: bool,
    /// Print tty debug output to the console
    #[clap(short, long)]
    debug: bool,
}

fn main() {
    env_logger::builder().format_timestamp(None).init();

    let args = PsxEmuArgs::parse();

    let event_loop = EventLoop::new();
    let mut display = if args.windowed {
        VkDisplay::windowed(&event_loop, args.vram)
    } else {
        VkDisplay::headless()
    };

    let mut psx = Psx::new(
        &args.bios,
        args.disk_file,
        PsxConfig {
            stdout_debug: args.debug,
        },
        display.device.clone(),
        display.queue.clone(),
    )
    .unwrap();

    let mut last_frame_time = Instant::now();
    let mut render_done = false;

    let mut audio_player = AudioPlayer::new(44100);
    if args.audio {
        audio_player.play();
    }

    event_loop.run(move |event, _target, control_flow| {
        if let Event::WindowEvent { event, .. } = event {
            match event {
                WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
                    return;
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
                            // Pause CPU and enable debug
                            Some(VirtualKeyCode::Slash) => psx.pause_cpu(),
                            Some(VirtualKeyCode::V) => display.toggle_full_vram_display(),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        *control_flow = ControlFlow::Poll;

        if render_done {
            if last_frame_time.elapsed().as_micros() < 16667 {
                return;
            }
            render_done = false;
            last_frame_time = Instant::now();
        }

        // Run the CPU for 100000 cycles, this allows for some time for UI
        // to be responsive and not spend the time on emulation alone
        // A full frame is generally around 564480 cycles
        if psx.clock_based_on_audio(100000) {
            display.render_frame(&mut psx);
            render_done = true;
        }
        let audio_buffer = psx.take_audio_buffer();
        if args.audio {
            audio_player.queue(&audio_buffer);
        }
    });
}
