use super::BusLine;

bitflags::bitflags! {
    #[derive(Default)]
    struct ChannelControl: u32 {
        const DIRECTION                = 0b00000000000000000000000000000001;
        const ADDRESS_STEP_DIRECTION   = 0b00000000000000000000000000000010;
        const CHOPPING_ENABLED         = 0b00000000000000000000000100000000;
        const SYNC_MODE                = 0b00000000000000000000011000000000;
        const CHOPPING_DMA_WINDOW_SIZE = 0b00000000000001110000000000000000;
        const CHOPPING_CPU_WINDOW_SIZE = 0b00000000011100000000000000000000;
        const START_BUSY               = 0b00000001000000000000000000000000;
        const START_TRIGGER            = 0b00010000000000000000000000000000;
        const UNKNOWN1                 = 0b00100000000000000000000000000000;
        const UNKNOWN2                 = 0b01000000000000000000000000000000;
        // const NOT_USED              = 0b10001110100010001111100011111100;
    }
}

impl ChannelControl {
    fn address_step(&self) -> i32 {
        if self.intersects(Self::ADDRESS_STEP_DIRECTION) {
            -4
        } else {
            4
        }
    }

    fn sync_mode(&self) -> u32 {
        (self.bits & Self::SYNC_MODE.bits) >> 9
    }
}

#[derive(Default)]
struct DmaChannel {
    base_address: u32,
    block_control: u32,
    channel_control: ChannelControl,
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
                let out = self.channel_control.bits;
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
                self.base_address = data & 0xFFFFFF;
            }
            0x4 => {
                log::info!("Dma channel block control {:08X}", data);
                self.block_control = data;
            }
            0x8 => {
                log::info!("Dma channel control write {:08X}", data);
                self.channel_control = ChannelControl::from_bits_truncate(data);
                log::info!("Dma channel control write {:?}", self.channel_control);
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
    fn perform_gpu_channel2_dma(&mut self, dma_bus: &mut super::DmaBus) {
        let channel = &mut self.channels[2];
        // GPU channel
        match channel.channel_control.sync_mode() {
            1 => {
                // Gpu VRAM load/store
                let direction_from_main_ram = channel
                    .channel_control
                    .intersects(ChannelControl::DIRECTION);
                let address_step = channel.channel_control.address_step();

                let block_size = (channel.block_control & 0xFFFF).max(0x10);
                let blocks = channel.block_control >> 16;
                // cannot overflow as it is 0x10 * 0xFFFF at max
                let mut remaining_length = block_size * blocks;

                let mut address = channel.base_address & 0xFFFFFC;

                let next_address = |remaining_length: &mut u32, address: &mut u32| {
                    *remaining_length -= 1;
                    let (r, overflow) = (*address as i32).overflowing_add(address_step);
                    assert!(!overflow);
                    *address = r as u32;
                };

                if direction_from_main_ram {
                    while remaining_length > 0 {
                        let data = dma_bus.main_ram.read_u32(address);
                        dma_bus.gpu.write_u32(0, data);
                        // step
                        next_address(&mut remaining_length, &mut address);
                    }
                } else {
                    todo!()
                }
            }
            2 => {
                // Linked list mode, to sending GP0 commands
                assert!(channel.channel_control.address_step() == 4);
                let mut linked_entry_addr = channel.base_address & 0xFFFFFC;
                while linked_entry_addr != 0xFFFFFF {
                    let linked_list_data = dma_bus.main_ram.read_u32(linked_entry_addr & 0xFFFFFC);
                    let n_entries = linked_list_data >> 24;
                    log::info!(
                        "got {} entries, from data {:08X} located at address {:08X}",
                        n_entries,
                        linked_list_data,
                        linked_entry_addr
                    );

                    for i in 1..(n_entries + 1) {
                        let cmd = dma_bus.main_ram.read_u32(linked_entry_addr + i * 4);
                        // gp0 command
                        // TODO: make sure that `gp1(04h)` is set to 2
                        dma_bus.gpu.write_u32(0, cmd);
                    }

                    linked_entry_addr = linked_list_data & 0xFFFFFF;
                }

                channel.base_address = 0xFFFFFF;
            }
            _ => unreachable!(),
        }
    }

    #[allow(dead_code)]
    pub(super) fn clock_dma(&mut self, dma_bus: &mut super::DmaBus) {
        // TODO: handle priority appropriately
        for (i, channel) in self.channels.iter_mut().enumerate() {
            let channel_enabled = (self.control >> (i * 4)) & 0b1000 != 0;

            if channel_enabled
                && channel
                    .channel_control
                    .intersects(ChannelControl::START_BUSY)
            {
                log::info!("channel {} doing DMA", i);
                // end transfer (remove busy bits)
                channel
                    .channel_control
                    .remove(ChannelControl::START_BUSY | ChannelControl::START_TRIGGER);

                match i {
                    2 => self.perform_gpu_channel2_dma(dma_bus),
                    6 => {
                        // GPU channel (OTC)

                        // must be to main ram
                        assert!(!channel
                            .channel_control
                            .intersects(ChannelControl::DIRECTION));
                        // must be backwards
                        assert!(channel.channel_control.address_step() == -4);
                        // must be sync mode 0
                        assert!(channel.channel_control.bits & ChannelControl::SYNC_MODE.bits == 0);

                        // word align
                        let mut current = channel.base_address & 0xFFFFFC;
                        let mut n_blocks = channel.block_control & 0xFFFF;
                        if n_blocks == 0 {
                            n_blocks = 0x10000;
                        }

                        // TODO: check if we should add one more linked list entry
                        for _ in 0..(n_blocks - 1) {
                            let next = current - 4;
                            // write a pointer to the next address
                            dma_bus.main_ram.write_u32(current, next);
                            current = next;
                        }
                        dma_bus.main_ram.write_u32(current, 0xFFFFFF);
                    }
                    _ => todo!(),
                }

                // handle only one DMA at a time
                break;
            }
        }
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
