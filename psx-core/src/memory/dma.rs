use super::interrupts::InterruptRequester;
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

bitflags::bitflags! {
    #[derive(Default)]
    struct DmaInterruptRegister: u32 {
        const UNKNOWN                = 0b00000000000000000000000000111111;
        const FORCE_IRQ              = 0b00000000000000001000000000000000;
        const IRQ_ENABLE             = 0b00000000011111110000000000000000;
        const IRQ_MASTER_ENABLE      = 0b00000000100000000000000000000000;
        const IRQ_FLAGS              = 0b01111111000000000000000000000000;
        const IRQ_MASTER_FLAG        = 0b10000000000000000000000000000000;
        // const NOT_USED            = 0b00000000000000000111111111000000;
    }
}

impl DmaInterruptRegister {
    #[inline]
    fn master_flag(&self) -> bool {
        self.intersects(Self::IRQ_MASTER_FLAG)
    }

    #[inline]
    fn request_interrupt(&mut self, channel: u32) {
        assert!(channel < 7);

        log::info!("requesting interrupt channel {}", channel);
        self.bits |= 1 << (channel + 24);
    }

    #[inline]
    fn compute_irq_master_flag(&self) -> bool {
        self.intersects(DmaInterruptRegister::FORCE_IRQ)
            || (self.intersects(DmaInterruptRegister::IRQ_MASTER_ENABLE)
                && (((self.bits & DmaInterruptRegister::IRQ_ENABLE.bits) >> 16)
                    & ((self.bits & DmaInterruptRegister::IRQ_FLAGS.bits) >> 24)
                    != 0))
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

    fn in_progress(&self) -> bool {
        self.intersects(Self::START_BUSY)
    }

    fn finish_transfer(&mut self) {
        self.remove(Self::START_BUSY)
    }
}

#[derive(Default)]
struct DmaChannel {
    // TODO: delay stuff are temporary to fix the DMA timing
    delay: u32,
    during_delay: bool,
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
    interrupt: DmaInterruptRegister,

    channels: [DmaChannel; 7],
}

impl Default for Dma {
    fn default() -> Self {
        Self {
            control: 0x07654321,
            interrupt: Default::default(),
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
                // transfer one block only
                let mut remaining_length = block_size;

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

                let blocks = blocks - 1;

                channel.block_control &= 0xFFFF;
                channel.block_control |= blocks << 16;
                channel.base_address = address;
                if blocks == 0 {
                    channel.channel_control.finish_transfer();
                    self.interrupt.request_interrupt(2);
                }
            }
            2 => {
                if channel.during_delay {
                    assert!(channel.delay > 0);
                    channel.delay -= 1;
                    if channel.delay == 0 {
                        channel.during_delay = false;
                        channel.channel_control.finish_transfer();
                        self.interrupt.request_interrupt(2);
                    }
                    return;
                }

                // Linked list mode, to sending GP0 commands
                assert!(channel.channel_control.address_step() == 4);
                let mut linked_entry_addr = channel.base_address & 0xFFFFFC;

                let mut linked_list_data = dma_bus.main_ram.read_u32(linked_entry_addr);
                let mut n_entries = linked_list_data >> 24;
                // make sure the GPU can handle this entry
                log::info!(
                    "got {} entries, from data {:08X} located at address {:08X}",
                    n_entries,
                    linked_list_data,
                    linked_entry_addr
                );
                assert!(n_entries < 16);

                while n_entries == 0 && linked_list_data & 0xFFFFFF != 0xFFFFFF {
                    linked_entry_addr = linked_list_data & 0xFFFFFC;
                    linked_list_data = dma_bus.main_ram.read_u32(linked_entry_addr);
                    n_entries = linked_list_data >> 24;

                    if n_entries != 0 {
                        channel.base_address = linked_entry_addr & 0xFFFFFF;
                        return;
                    }

                    log::info!(
                        "skipping: got {} entries, from data {:08X} located at address {:08X}",
                        n_entries,
                        linked_list_data,
                        linked_entry_addr
                    );
                }
                channel.delay += n_entries;

                for i in 1..(n_entries + 1) {
                    let cmd = dma_bus.main_ram.read_u32(linked_entry_addr + i * 4);
                    // gp0 command
                    // TODO: make sure that `gp1(04h)` is set to 2
                    dma_bus.gpu.write_u32(0, cmd);
                }

                channel.base_address = linked_list_data & 0xFFFFFF;

                if channel.base_address == 0xFFFFFF {
                    channel.during_delay = true;
                }
            }
            _ => unreachable!(),
        }
    }

    fn perform_otc_channel6_dma(&mut self, dma_bus: &mut super::DmaBus) {
        let channel = &mut self.channels[6];
        // must be to main ram
        assert!(!channel
            .channel_control
            .intersects(ChannelControl::DIRECTION));
        // must be backwards
        assert!(channel.channel_control.address_step() == -4);
        // must be sync mode 0
        assert!(channel.channel_control.sync_mode() == 0);
        // make sure there is no chopping, so we can finish this in one go
        assert!(!channel
            .channel_control
            .intersects(ChannelControl::CHOPPING_ENABLED));

        // word align
        let mut current = channel.base_address & 0xFFFFFC;
        let mut n_entries = channel.block_control & 0xFFFF;
        if n_entries == 0 {
            n_entries = 0x10000;
        }

        // TODO: check if we should add one more linked list entry
        for _ in 0..(n_entries - 1) {
            let next = current - 4;
            // write a pointer to the next address
            dma_bus.main_ram.write_u32(current, next);
            current = next;
        }
        dma_bus.main_ram.write_u32(current, 0xFFFFFF);
        channel.channel_control.finish_transfer();
        self.interrupt.request_interrupt(6);
    }

    #[allow(dead_code)]
    pub(super) fn clock_dma(
        &mut self,
        dma_bus: &mut super::DmaBus,
        interrupt_requester: &mut impl InterruptRequester,
    ) {
        // TODO: handle priority appropriately
        for (i, channel) in self.channels.iter_mut().enumerate() {
            let channel_enabled = (self.control >> (i * 4)) & 0b1000 != 0;

            if channel_enabled && channel.channel_control.in_progress() {
                log::info!("channel {} doing DMA", i);
                // end transfer (remove busy bits)
                channel
                    .channel_control
                    .remove(ChannelControl::START_TRIGGER);

                match i {
                    2 => self.perform_gpu_channel2_dma(dma_bus),
                    6 => self.perform_otc_channel6_dma(dma_bus),
                    _ => todo!(),
                }

                // handle only one DMA at a time
                break;
            }
        }

        let new_master_flag = self.interrupt.compute_irq_master_flag();
        // only in transition from false to true, so it should be false now
        if new_master_flag && !self.interrupt.master_flag() {
            interrupt_requester.request_dma();
        }

        self.interrupt
            .set(DmaInterruptRegister::IRQ_MASTER_FLAG, new_master_flag);
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
            0xF4 => self.interrupt.bits,
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
                // we will keep the upper-most bit
                let old_interrupt = self.interrupt.bits;
                let new_data = data & 0xFFFFFF;
                // and the flags will be reset on write
                let irq_flags_reset = data & 0x7F000000;
                let new_interrupt = ((old_interrupt & 0xFF000000) & !irq_flags_reset) | new_data;

                self.interrupt = DmaInterruptRegister::from_bits_truncate(new_interrupt);
                log::info!(
                    "DMA interrupt input: {:08X}, result: {:08X}, {:?}",
                    data,
                    self.interrupt.bits,
                    self.interrupt
                );
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
