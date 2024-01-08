/// Copied from `mizu`
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::{HeapProducer, HeapRb};

pub enum BufferFlowState {
    Normal,
    Overflow,
    Underflow,
}

pub struct AudioPlayer {
    buffer_producer: HeapProducer<i16>,
    output_stream: cpal::Stream,
    buffer_state: BufferFlowState,
}

impl AudioPlayer {
    pub fn new(sample_rate: u32) -> Self {
        let host = cpal::default_host();
        let output_device = host
            .default_output_device()
            .expect("failed to get default output audio device");

        let config = cpal::StreamConfig {
            channels: 2,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        // Limiting the number of samples in the buffer is better to minimize
        // audio delay in emulation, this is because emulation speed
        // does not 100% match audio playing speed (44100Hz).
        // The buffer holds only audio for 1/4 second, which is good enough for delays,
        // It can be reduced more, but it might cause noise(?) for slower machines
        // or if any CPU intensive process started while the emulator is running
        let buffer = HeapRb::new(sample_rate as usize / 4);
        let (buffer_producer, mut buffer_consumer) = buffer.split();

        let output_data_fn = move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
            for sample in data {
                *sample = buffer_consumer.pop().unwrap_or(0);
            }
        };

        let output_stream = output_device
            .build_output_stream(&config, output_data_fn, Self::err_fn, None)
            .expect("failed to build an output audio stream");

        Self {
            buffer_producer,
            output_stream,
            buffer_state: BufferFlowState::Normal,
        }
    }

    pub fn play(&self) {
        self.output_stream.play().unwrap();
    }

    /// Pause the player
    /// > not used for now, but maybe later
    #[allow(dead_code)]
    pub fn pause(&self) {
        self.output_stream.pause().unwrap();
    }

    pub fn queue(&mut self, data: &[i16]) {
        if self.buffer_producer.capacity() - self.buffer_producer.len() < data.len() {
            self.buffer_state = BufferFlowState::Overflow;
        }
        if self.buffer_producer.len() < data.len() / 2 {
            self.buffer_state = BufferFlowState::Underflow;
        }
        self.buffer_producer.push_slice(data);
    }

    pub fn take_buffer_state(&mut self) -> BufferFlowState {
        std::mem::replace(&mut self.buffer_state, BufferFlowState::Normal)
    }
}

impl AudioPlayer {
    fn err_fn(err: cpal::StreamError) {
        eprintln!("an error occurred on audio stream: {}", err);
    }
}
