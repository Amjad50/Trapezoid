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

/// All Dma handles are of type `fn(&mut DmaChannel, &mut super::DmaBus) -> (u32, bool)`
/// The return values are `(The number of cpu cycles spent, Is dma finished)`
impl Dma {
    fn perform_mdec_in_channel0_dma(
        channel: &mut DmaChannel,
        dma_bus: &mut super::DmaBus,
    ) -> (u32, bool) {
        // must be from main ram
        assert!(channel
            .channel_control
            .intersects(ChannelControl::DIRECTION));
        // must be forward
        assert!(channel.channel_control.address_step() == 4);
        // must be sync mode 1
        assert!(channel.channel_control.sync_mode() == 1);
        // make sure there is no chopping, so we can finish this in one go
        // TODO: implement chopping
        assert!(!channel
            .channel_control
            .intersects(ChannelControl::CHOPPING_ENABLED));

        // TODO: check if the max is 32 or not
        let block_size = channel.block_control & 0xFFFF;
        let blocks = channel.block_control >> 16;

        // word align
        let mut address = channel.base_address & 0xFFFFFC;

        for _ in 0..block_size {
            let data = dma_bus.main_ram.read_u32(address);
            // TODO: write to params directly
            dma_bus.mdec.write_u32(0, data);

            // step
            address += 4;
        }

        let blocks = blocks - 1;

        channel.block_control &= 0xFFFF;
        channel.block_control |= blocks << 16;
        channel.base_address = address;

        (block_size, blocks == 0)
    }

    fn perform_gpu_channel2_dma(
        channel: &mut DmaChannel,
        dma_bus: &mut super::DmaBus,
    ) -> (u32, bool) {
        // GPU channel
        match channel.channel_control.sync_mode() {
            1 => {
                // Gpu VRAM load/store
                let direction_from_main_ram = channel
                    .channel_control
                    .intersects(ChannelControl::DIRECTION);
                let address_step = channel.channel_control.address_step();

                // TODO: check if the max is 16 or not
                let block_size = channel.block_control & 0xFFFF;
                let blocks = channel.block_control >> 16;

                let mut address = channel.base_address & 0xFFFFFC;

                if direction_from_main_ram {
                    for _ in 0..block_size {
                        let data = dma_bus.main_ram.read_u32(address);
                        dma_bus.gpu.write_u32(0, data);
                        // step
                        address = (address as i32 + address_step as i32) as u32;
                    }
                } else {
                    for _ in 0..block_size {
                        let data = dma_bus.gpu.read_u32(0);
                        dma_bus.main_ram.write_u32(address, data);
                        // step
                        address = (address as i32 + address_step as i32) as u32;
                    }
                }

                let blocks = blocks - 1;

                channel.block_control &= 0xFFFF;
                channel.block_control |= blocks << 16;
                channel.base_address = address;

                (block_size, blocks == 0)
            }
            2 => {
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
                        channel.base_address = linked_entry_addr & 0xFFFFFC;
                        // return, so we can start again from the beginning
                        // TODO: should we just continue?
                        return (0, false);
                    }

                    log::info!(
                        "skipping: got {} entries, from data {:08X} located at address {:08X}",
                        n_entries,
                        linked_list_data,
                        linked_entry_addr
                    );
                }

                for i in 1..(n_entries + 1) {
                    let cmd = dma_bus.main_ram.read_u32(linked_entry_addr + i * 4);
                    // gp0 command
                    // TODO: make sure that `gp1(04h)` is set to 2
                    dma_bus.gpu.write_u32(0, cmd);
                }

                channel.base_address = linked_list_data & 0xFFFFFF;

                (n_entries + 1, channel.base_address == 0xFFFFFF)
            }
            _ => unreachable!(),
        }
    }

    fn perform_cdrom_channel3_dma(
        channel: &mut DmaChannel,
        dma_bus: &mut super::DmaBus,
    ) -> (u32, bool) {
        // must be to main ram
        assert!(!channel
            .channel_control
            .intersects(ChannelControl::DIRECTION));
        // must be forward
        assert!(channel.channel_control.address_step() == 4);
        // must be sync mode 0
        assert!(channel.channel_control.sync_mode() == 0);
        // make sure there is no chopping, so we can finish this in one go
        // TODO: implement chopping
        assert!(!channel
            .channel_control
            .intersects(ChannelControl::CHOPPING_ENABLED));

        // must be triggered manually
        if !channel
            .channel_control
            .intersects(ChannelControl::START_TRIGGER)
        {
            return (0, true);
        }

        let block_size = channel.block_control & 0xFFFF;
        let blocks = channel.block_control >> 16;
        log::info!("CD-ROM DMA: block size: {:04X}", block_size);
        assert!(blocks == 1);

        // word align
        let mut address = channel.base_address & 0xFFFFFC;

        for _ in 0..block_size {
            // DATA FIFO
            // read u32
            let mut data = dma_bus.cdrom.read_u8(2) as u32;
            data |= (dma_bus.cdrom.read_u8(2) as u32) << 8;
            data |= (dma_bus.cdrom.read_u8(2) as u32) << 16;
            data |= (dma_bus.cdrom.read_u8(2) as u32) << 24;

            dma_bus.main_ram.write_u32(address, data);

            // step
            address += 4;
        }

        // TODO: is it ok to clear this?
        channel.block_control = 0;
        channel.base_address = address;

        // chrom transfer rate:
        // BIOS: 24 clk/word
        // GAMES: 40 clk/word
        //
        // Not sure exactly what is BIOS and what is GAMES, so for now, lets make
        // it 30 clk/word (around the middle).
        (block_size * 30, true)
    }

    // TODO: implement this, now its just an empty handler that trigger interrupt
    fn perform_spu_channel4_dma(
        channel: &mut DmaChannel,
        _dma_bus: &mut super::DmaBus,
    ) -> (u32, bool) {
        // must be sync mode 1
        assert!(channel.channel_control.sync_mode() == 1);
        // must be from main ram (for now, TODO: fix this)
        assert!(channel
            .channel_control
            .intersects(ChannelControl::DIRECTION));
        // must be forward
        assert!(channel.channel_control.address_step() == 4);
        // TODO: implement chopping
        assert!(!channel
            .channel_control
            .intersects(ChannelControl::CHOPPING_ENABLED));

        // TODO: check if the max is 16 or not
        let block_size = channel.block_control & 0xFFFF;
        let blocks = channel.block_control >> 16;

        let mut address = channel.base_address & 0xFFFFFC;

        // transfer one block only
        address += 4 * block_size;

        let blocks = blocks - 1;

        channel.block_control &= 0xFFFF;
        channel.block_control |= blocks << 16;
        channel.base_address = address;

        (0, blocks == 0)
    }

    // Some control flags are ignored here like:
    // - From RAM (Direction)
    // - Forward address step
    // - Chopping
    // - sync mode
    // Becuase they are hardwired
    fn perform_otc_channel6_dma(
        channel: &mut DmaChannel,
        dma_bus: &mut super::DmaBus,
    ) -> (u32, bool) {
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

        // must be triggered manually
        if !channel
            .channel_control
            .intersects(ChannelControl::START_TRIGGER)
        {
            return (0, true);
        }

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

        (n_entries, true)
    }
}

impl Dma {
    pub(super) fn needs_to_run(&self) -> bool {
        self.channels.iter().enumerate().any(|(i, channel)| {
            let channel_enabled = (self.control >> (i * 4)) & 0b1000 != 0;

            channel_enabled && channel.channel_control.in_progress()
        })
    }

    #[allow(dead_code)]
    pub(super) fn clock_dma(
        &mut self,
        dma_bus: &mut super::DmaBus,
        interrupt_requester: &mut impl InterruptRequester,
    ) -> u32 {
        // record the number of cycles that are spent of the cpu
        let mut cpu_cycles = 0;

        // TODO: handle priority appropriately
        for (i, channel) in self.channels.iter_mut().enumerate() {
            let channel_enabled = (self.control >> (i * 4)) & 0b1000 != 0;

            if channel_enabled && channel.channel_control.in_progress() {
                log::info!("channel {} doing DMA", i);

                let (cycles_to_delay, finished) = match i {
                    0 => Self::perform_mdec_in_channel0_dma(channel, dma_bus),
                    2 => Self::perform_gpu_channel2_dma(channel, dma_bus),
                    3 => Self::perform_cdrom_channel3_dma(channel, dma_bus),
                    4 => Self::perform_spu_channel4_dma(channel, dma_bus),
                    6 => Self::perform_otc_channel6_dma(channel, dma_bus),
                    _ => todo!("DMA channel {}", i),
                };

                cpu_cycles += cycles_to_delay;
                if finished {
                    channel.channel_control.finish_transfer();
                    self.interrupt.request_interrupt(i as u32);
                }

                // remove trigger afterwards, since some handlers might check
                //  for manual trigger
                channel
                    .channel_control
                    .remove(ChannelControl::START_TRIGGER);

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

        cpu_cycles
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

    fn write_u32(&mut self, addr: u32, mut data: u32) {
        match addr {
            0x80..=0xEF => {
                let channel_index = (addr >> 4) - 8;
                log::info!("DMA, writing to channel {}", channel_index);

                // hardwired some control for channel 6
                // TODO: maybe rewrite channels in individual structs?
                //  for special cases
                if channel_index == 6 && addr & 0xF == 8 {
                    // keep only START_TRIGGER | START_BUSY | UNKNOWN2
                    // and hardwired the rest to zero
                    data &= 0b0101_0001_0000_0000_0000_0000_0000_0000;
                    // hardware the address direction to backward
                    data |= 2;
                }

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
