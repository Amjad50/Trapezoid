//! A rendering backend that does nothing. No attempts at rendering anything here. It just stubs
//! out what parts of vulkano are needed to compile the rest of the code.

use crate::gpu::BackendCommand;
pub struct Device;
pub struct Queue;

impl Queue {
    pub fn queue_family_index(&self) -> u32 {
        0
    }
}

pub struct Image;
pub struct GpuContext;
pub struct StandardCommandBufferAllocator;

impl StandardCommandBufferAllocator {
    pub fn new(_device: Arc<Device>, _create_info: ()) -> Self {
        Self {}
    }
}

pub struct PrimaryAutoCommandBuffer;
pub struct AutoCommandBufferBuilder<L> {
    l: std::marker::PhantomData<L>,
}

impl<L> AutoCommandBufferBuilder<L> {
    pub fn primary(
        _allocator: &StandardCommandBufferAllocator,
        _index: u32,
        _cbu: CommandBufferUsage,
    ) -> Result<Self, ()> {
        Ok(Self {
            l: Default::default(),
        })
    }
    pub fn blit_image(&self, _blit_image_info: BlitImageInfo) -> Option<()> {
        Some(())
    }
    pub fn build(&self) -> Option<Self> {
        Some(Self {
            l: Default::default(),
        })
    }
}

pub trait GpuFuture {
    fn then_execute(
        &self,
        _q: Arc<Queue>,
        _cb: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer>,
    ) -> Option<()> {
        Some(())
    }
}

pub struct BlitImageInfo {
    pub src_image: Arc<Image>,
    pub src_image_layout: ImageLayout,
    pub dst_image: Arc<Image>,
    pub dst_image_layout: ImageLayout,
    pub filter: Filter,
}

impl BlitImageInfo {
    pub fn images(_src_image: Arc<Image>, _dst_image: Arc<Image>) -> BlitImageInfo {
        unimplemented!()
    }
}

pub struct ImageBlit;

pub enum Filter {
    Nearest,
}
pub enum ImageLayout {}

pub enum CommandBufferUsage {
    OneTimeSubmit,
}

use crate::gpu::GpuStat;

use crossbeam::{
    atomic::AtomicCell,
    channel::{Receiver, Sender},
};
use std::sync::Arc;

pub struct GpuBackend {
    gpu_context: GpuContext,
    gpu_stat: Arc<AtomicCell<GpuStat>>,

    gpu_read_sender: Sender<u32>,
    gpu_backend_receiver: Receiver<BackendCommand>,
}

impl GpuBackend {
    pub(super) fn start(
        _device: Arc<Device>,
        _queue: Arc<Queue>,
        _gpu_stat: Arc<AtomicCell<GpuStat>>,
        _gpu_read_sender: Sender<u32>,
        _gpu_backend_receiver: Receiver<BackendCommand>,
        _gpu_front_image_sender: Sender<Arc<Image>>,
    ) -> std::thread::JoinHandle<()> {
        // TODO: We need to return a JoinHandle<()>, but we don't actually need to do spawn any
        // threads. Is there a way to not spawn this stupid do nothing thread?
        std::thread::spawn(|| {})
    }

    #[allow(unused_mut)]
    fn run(mut self) {}
}
