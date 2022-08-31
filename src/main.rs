use std::{path::PathBuf, sync::Arc, time::Instant};

use psx_core::{DigitalControllerKey, Psx};

use clap::Parser;
use vulkano::{
    device::{
        physical::{PhysicalDevice, PhysicalDeviceType},
        Device, DeviceCreateInfo, DeviceExtensions, Queue, QueueCreateInfo,
    },
    image::{ImageUsage, SwapchainImage},
    instance::{Instance, InstanceCreateInfo, InstanceExtensions},
    swapchain::{
        self, AcquireError, CompositeAlpha, PresentMode, Surface, Swapchain, SwapchainCreateInfo,
        SwapchainCreationError,
    },
    sync::{self, GpuFuture},
};
use vulkano_win::VkSurfaceBuild;
use winit::{
    event::{ElementState, Event, VirtualKeyCode, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::{Window, WindowBuilder},
};

enum DisplayType {
    Windowed {
        surface: Arc<Surface<Window>>,
        swapchain: Arc<Swapchain<Window>>,
        images: Vec<Arc<SwapchainImage<Window>>>,
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
        let required_extensions = vulkano_win::required_extensions();

        let instance = Instance::new(InstanceCreateInfo {
            enabled_extensions: required_extensions,
            ..Default::default()
        })
        .unwrap();

        let surface = WindowBuilder::new()
            .build_vk_surface(event_loop, instance.clone())
            .unwrap();

        let device_extensions = DeviceExtensions {
            khr_swapchain: true,
            ..DeviceExtensions::none()
        };

        let (physical_device, queue_family) = PhysicalDevice::enumerate(&instance)
            .filter(|&p| p.supported_extensions().is_superset_of(&device_extensions))
            .filter_map(|p| {
                p.queue_families()
                    .find(|&q| {
                        q.supports_graphics() && q.supports_surface(&surface).unwrap_or(false)
                    })
                    .map(|q| (p, q))
            })
            .min_by_key(|(p, _)| match p.properties().device_type {
                PhysicalDeviceType::DiscreteGpu => 0,
                PhysicalDeviceType::IntegratedGpu => 1,
                PhysicalDeviceType::VirtualGpu => 2,
                PhysicalDeviceType::Cpu => 3,
                PhysicalDeviceType::Other => 4,
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
                enabled_extensions: physical_device
                    .required_extensions()
                    .union(&device_extensions),
                queue_create_infos: vec![QueueCreateInfo::family(queue_family)],
                ..Default::default()
            },
        )
        .unwrap();

        let queue = queues.next().unwrap();

        let (swapchain, images) = {
            let caps = physical_device
                .surface_capabilities(&surface, Default::default())
                .unwrap();

            let format = Some(
                physical_device
                    .surface_formats(&surface, Default::default())
                    .unwrap()[0]
                    .0,
            );

            let dimensions: [u32; 2] = surface.window().inner_size().into();

            Swapchain::new(
                device.clone(),
                surface.clone(),
                SwapchainCreateInfo {
                    min_image_count: caps.min_image_count,
                    image_format: format,
                    image_extent: dimensions,
                    image_usage: ImageUsage {
                        transfer_destination: true,
                        ..ImageUsage::none()
                    },
                    composite_alpha: CompositeAlpha::Opaque,
                    present_mode: PresentMode::Immediate,
                    ..Default::default()
                },
            )
            .unwrap()
        };

        Self {
            device,
            queue,
            display_type: DisplayType::Windowed {
                surface,
                swapchain,
                images,
                full_vram_display,
                last_frame_time: Instant::now(),
            },
        }
    }

    fn headless() -> Self {
        let instance = Instance::new(InstanceCreateInfo {
            enabled_extensions: InstanceExtensions::none(),
            ..Default::default()
        })
        .unwrap();

        let (physical_device, queue_family) = PhysicalDevice::enumerate(&instance)
            .filter_map(|p| {
                p.queue_families()
                    .find(|&q| q.supports_graphics())
                    .map(|q| (p, q))
            })
            .min_by_key(|(p, _)| match p.properties().device_type {
                PhysicalDeviceType::DiscreteGpu => 0,
                PhysicalDeviceType::IntegratedGpu => 1,
                PhysicalDeviceType::VirtualGpu => 2,
                PhysicalDeviceType::Cpu => 3,
                PhysicalDeviceType::Other => 4,
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
                enabled_extensions: *physical_device.required_extensions(),
                queue_create_infos: vec![QueueCreateInfo::family(queue_family)],
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
                let dimensions: [u32; 2] = surface.window().inner_size().into();
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
                last_frame_time,
            } => {
                surface.window().set_title(&format!(
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

                let current_image = images[image_num].clone();

                psx.blit_to_front(current_image, *full_vram_display, acquire_future);

                sync::now(self.device.clone())
                    .then_swapchain_present(self.queue.clone(), swapchain.clone(), image_num)
                    .flush()
                    .unwrap();
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
    /// Display the full vram
    #[clap(short, long)]
    vram: bool,
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
        display.device.clone(),
        display.queue.clone(),
    )
    .unwrap();

    event_loop.run(move |event, _target, control_flow| {
        match event {
            Event::WindowEvent { event, .. } => match event {
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
                    }
                }
                _ => {}
            },
            Event::MainEventsCleared => {
                display.render_frame(&mut psx);
            }
            _ => {}
        }
        *control_flow = ControlFlow::Poll;
    });
}
