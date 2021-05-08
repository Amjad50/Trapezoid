use super::BusLine;

#[derive(Default)]
struct DmaChannel {
    base_address: u32,
    block_control: u32,
    channel_control: u32,
}

impl DmaChannel {
    fn read(&mut self, addr: u32) -> u32 {
        match addr {
            0x0 => {
                let out = self.base_address;
                log::info!("Dma base address read {:08X}", out);
                out
            }
            0x4 => {
                let out = self.block_control;
                log::info!("Dma block control read {:08X}", out);
                out
            }
            0x8 => {
                let out = self.channel_control;
                log::info!("Dma channel read control {:08X}", out);
                out
            }

            0xC => {
                log::info!("Dma channel read control mirror 0xC");
                self.read(0x8)
            }
            _ => unreachable!(),
        }
    }

    fn write(&mut self, addr: u32, data: u32) {
        match addr {
            0x0 => {
                log::info!("Dma channel base address {:08X}", data);
                self.base_address = data;
            }
            0x4 => {
                log::info!("Dma channel block control {:08X}", data);
                self.block_control = data;
            }
            0x8 => {
                log::info!("Dma channel control write {:08X}", data);
                self.channel_control = data;
                if self.channel_control & 0x1100000 != 0 {
                    log::info!("Dma started and completed");
                    // simulate DMA completion
                    self.channel_control &= !0x1100000;
                }
            }
            // mirror
            0xC => {
                log::info!("Dma channel control write mirror 0xC");
                self.write(0x8, data)
            }
            _ => unreachable!(),
        }
    }
}

pub struct Dma {
    control: u32,
    interrupt: u32,

    channels: [DmaChannel; 7],
}

impl Default for Dma {
    fn default() -> Self {
        Self {
            control: 0x07654321,
            interrupt: 0,
            channels: Default::default(),
        }
    }
}

impl Dma {
    #[allow(dead_code)]
    pub(super) fn clock_dma(&mut self, _dma_bus: &mut super::DmaBus) {
        todo!("Handle DMA transfer")
    }
}

impl BusLine for Dma {
    fn read_u32(&mut self, addr: u32) -> u32 {
        match addr {
            0x80..=0xEF => {
                let channel_index = (addr >> 4) - 8;
                log::info!("DMA, reading from channel {}", channel_index);
                self.channels[channel_index as usize].read(addr & 0xF)
            }
            0xF0 => self.control,
            0xF4 => self.interrupt,
            _ => unreachable!(),
        }
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        match addr {
            0x80..=0xEF => {
                let channel_index = (addr >> 4) - 8;
                log::info!("DMA, writing to channel {}", channel_index);
                self.channels[channel_index as usize].write(addr & 0xF, data)
            }
            0xF0 => {
                log::info!("DMA control {:08X}", data);
                self.control = data
            }
            0xF4 => {
                log::info!("DMA interrupt {:08X}", data);
                self.interrupt = data
            }
            _ => unreachable!(),
        }
    }

    fn read_u16(&mut self, _addr: u32) -> u16 {
        todo!()
    }

    fn write_u16(&mut self, _addr: u32, _data: u16) {
        todo!()
    }

    fn read_u8(&mut self, _addr: u32) -> u8 {
        todo!()
    }

    fn write_u8(&mut self, _addr: u32, _data: u8) {
        todo!()
    }
}
