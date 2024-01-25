use crate::mdec;
use crate::memory::Result;

use super::interrupts::InterruptRequester;
use super::BusLine;

bitflags::bitflags! {
    #[derive(Default, Debug)]
    struct ChannelControl: u32 {
        const DIRECTION_FROM_RAM       = 0b00000000000000000000000000000001;
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
        (self.bits() & Self::SYNC_MODE.bits()) >> 9
    }

    fn in_progress(&self) -> bool {
        self.intersects(Self::START_BUSY)
    }

    fn finish_transfer(&mut self) {
        self.remove(Self::START_BUSY)
    }

    /// In words units
    fn chopping_dma_window_size(&self) -> u32 {
        1 << ((self.bits() & Self::CHOPPING_DMA_WINDOW_SIZE.bits()) >> 16)
    }

    /// In words units
    fn chopping_cpu_window_size(&self) -> u8 {
        1 << ((self.bits() & Self::CHOPPING_CPU_WINDOW_SIZE.bits()) >> 20)
    }
}

bitflags::bitflags! {
    #[derive(Default, Debug)]
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

        // only set if enabled
        if (self.bits() >> 16) & (1 << channel) != 0 {
            log::info!("requesting interrupt channel {}", channel);
            *self |= Self::from_bits_retain(1 << (channel + 24));
        }
    }

    #[inline]
    fn compute_irq_master_flag(&self) -> bool {
        self.intersects(DmaInterruptRegister::FORCE_IRQ)
            || (self.intersects(DmaInterruptRegister::IRQ_MASTER_ENABLE)
                && (((self.bits() & DmaInterruptRegister::IRQ_ENABLE.bits()) >> 16)
                    & ((self.bits() & DmaInterruptRegister::IRQ_FLAGS.bits()) >> 24)
                    != 0))
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
                let out = self.channel_control.bits();
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
                self.channel_control = ChannelControl::from_bits_retain(data);
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
            .intersects(ChannelControl::DIRECTION_FROM_RAM));
        // must be forward
        assert!(channel.channel_control.address_step() == 4);
        // must be sync mode 1
        assert!(channel.channel_control.sync_mode() == 1);
        // chopping is only for sync mode 0
        assert!(!channel
            .channel_control
            .intersects(ChannelControl::CHOPPING_ENABLED));

        // TODO: check if the max is 32 or not
        let block_size = channel.block_control & 0xFFFF;
        let blocks = channel.block_control >> 16;

        // word align
        let mut address = channel.base_address & 0xFFFFFC;

        for _ in 0..block_size {
            let data = dma_bus.main_ram.read_u32(address).unwrap();
            // TODO: write to params directly
            dma_bus.mdec.write_u32(0, data).unwrap();

            // step
            address += 4;
        }

        // NOTE: treat 0 as 1, and do not overflow
        let blocks = blocks.saturating_sub(1);

        channel.block_control &= 0xFFFF;
        channel.block_control |= blocks << 16;
        channel.base_address = address;

        (block_size, blocks == 0)
    }

    fn perform_mdec_out_channel1_dma(
        channel: &mut DmaChannel,
        dma_bus: &mut super::DmaBus,
    ) -> (u32, bool) {
        // must be to main ram
        assert!(!channel
            .channel_control
            .intersects(ChannelControl::DIRECTION_FROM_RAM));
        // must be forward
        assert!(channel.channel_control.address_step() == 4);
        // must be sync mode 1
        assert!(channel.channel_control.sync_mode() == 1);
        // chopping is only for sync mode 0
        assert!(!channel
            .channel_control
            .intersects(ChannelControl::CHOPPING_ENABLED));

        // TODO: check if the max is 32 or not
        let block_size = channel.block_control & 0xFFFF;
        let blocks = channel.block_control >> 16;

        // word align
        let mut address = channel.base_address & 0xFFFFFC;

        for _ in 0..block_size {
            // DOCS: If there's data in the output fifo, then the Current Block bits
            // are always set to the current output block number (ie. Y1..Y4; or
            // Y for mono) (this information is apparently passed to the DMA1
            // controller, so that it knows if and how it must re-order the data in RAM).
            //
            // Because the mdec is not running in sync with DMA, we store the
            // `Current Block` details for each fifo, and we can request them like this to compute
            // the location of the write.
            let fifo_state = dma_bus.mdec.fifo_current_state();
            // TODO: read whole buffer
            let data = dma_bus.mdec.read_fifo();

            // TODO: test for 24 bit mode
            // this specifies the re-order arrangement.
            //
            // In 24 bit mode, the data is arranged as follows:
            // each row in a single block is:
            // 111111
            // where each character is 1 word. Each row is 8 pixels, where each
            // pixel is 3 byte, thus 24 bytes per row (6 words).
            //
            // The rows of data will come one after another:
            // 111111 111111 ...
            // But the result we want is
            // 111111 222222 111111 ...
            // So the base of re-order is to split the blocks into 6 words chunks
            // and interlace them.
            //
            // This is the same for 15 bit mode but with 4 words instead of 6.
            let row_size = if fifo_state.is_24bit { 6 } else { 4 };
            // 8 pixels in height
            let block_size = row_size * 8;

            let offset = match fifo_state.block_type {
                mdec::BlockType::Y1 | mdec::BlockType::Y3 => {
                    let base_index = fifo_state.index as i32 / row_size;
                    base_index * row_size
                }
                mdec::BlockType::Y2 | mdec::BlockType::Y4 => {
                    let base_index = fifo_state.index as i32 / row_size;
                    base_index * row_size + row_size - block_size
                }
                mdec::BlockType::YCr => 0,
                _ => unreachable!(),
            };

            // The location of the write, this is result of the re-ordering
            // of the MDEC blocks.
            let effective_address = (address as i32 + (offset * 4)) as u32;
            dma_bus.main_ram.write_u32(effective_address, data).unwrap();

            // step
            address += 4;
        }

        // NOTE: treat 0 as 1, and do not overflow
        let blocks = blocks.saturating_sub(1);

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
                    .intersects(ChannelControl::DIRECTION_FROM_RAM);
                let address_step = channel.channel_control.address_step();

                // TODO: check if the max is 16 or not
                let block_size = channel.block_control & 0xFFFF;
                let blocks = channel.block_control >> 16;

                let mut address = channel.base_address & 0xFFFFFC;

                if direction_from_main_ram {
                    for _ in 0..block_size {
                        let data = dma_bus.main_ram.read_u32(address).unwrap();
                        dma_bus.gpu.write_u32(0, data).unwrap();
                        // step
                        address = (address as i32 + address_step) as u32;
                    }
                } else {
                    for _ in 0..block_size {
                        let data = dma_bus.gpu.read_u32(0).unwrap();
                        dma_bus.main_ram.write_u32(address, data).unwrap();
                        // step
                        address = (address as i32 + address_step) as u32;
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

                let mut linked_list_data = dma_bus.main_ram.read_u32(linked_entry_addr).unwrap();
                let mut n_entries = linked_list_data >> 24;
                // make sure the GPU can handle this entry
                log::info!(
                    "got {} entries, from data {:08X} located at address {:08X}",
                    n_entries,
                    linked_list_data,
                    linked_entry_addr
                );
                // The GPU only support 16 enteries, but noticed some games
                // use value higher than that. for us it doesn't really matter
                // as we can manage any number
                // assert!(n_entries < 16);

                while n_entries == 0 && linked_list_data & 0xFFFFFF != 0xFFFFFF {
                    linked_entry_addr = linked_list_data & 0xFFFFFC;
                    linked_list_data = dma_bus.main_ram.read_u32(linked_entry_addr).unwrap();
                    n_entries = linked_list_data >> 24;

                    if n_entries != 0 {
                        channel.base_address = linked_entry_addr & 0xFFFFFC;
                        // return, so we can start again from the beginning
                        // TODO: should we just continue?
                        return (0, false);
                    }

                    log::trace!(
                        "skipping: got {} entries, from data {:08X} located at address {:08X}",
                        n_entries,
                        linked_list_data,
                        linked_entry_addr
                    );
                }

                for i in 1..(n_entries + 1) {
                    let cmd = dma_bus
                        .main_ram
                        .read_u32(linked_entry_addr + i * 4)
                        .unwrap();
                    // gp0 command
                    // TODO: make sure that `gp1(04h)` is set to 2
                    dma_bus.gpu.write_u32(0, cmd).unwrap();
                }

                channel.base_address = linked_list_data & 0xFFFFFF;

                (n_entries + 1, channel.base_address == 0xFFFFFF)
            }
            _ => unreachable!("{}", channel.channel_control.sync_mode()),
        }
    }

    fn perform_cdrom_channel3_dma(
        channel: &mut DmaChannel,
        dma_bus: &mut super::DmaBus,
    ) -> (u32, bool) {
        // must be to main ram
        assert!(!channel
            .channel_control
            .intersects(ChannelControl::DIRECTION_FROM_RAM));
        // must be forward
        assert!(channel.channel_control.address_step() == 4);
        // must be sync mode 0
        assert!(channel.channel_control.sync_mode() == 0);

        // must be triggered manually
        if !channel
            .channel_control
            .intersects(ChannelControl::START_TRIGGER)
        {
            return (0, true);
        }

        let chopping = channel
            .channel_control
            .intersects(ChannelControl::CHOPPING_ENABLED);

        if chopping {
            log::info!(
                "chopping enabled: CPU window: {:02X}, DMA window: {:02X}",
                channel.channel_control.chopping_cpu_window_size(),
                channel.channel_control.chopping_dma_window_size()
            );
        }

        let mut block_size = channel.block_control & 0xFFFF;
        if block_size == 0 {
            block_size = 0x10000;
        }
        log::info!("CD-ROM DMA: block size: {:04X}", block_size);

        // word align
        let mut address = channel.base_address & 0xFFFFFC;

        for _ in 0..block_size {
            // DATA FIFO
            // read u32
            let mut data = dma_bus.cdrom.read_u8(2).unwrap() as u32;
            data |= (dma_bus.cdrom.read_u8(2).unwrap() as u32) << 8;
            data |= (dma_bus.cdrom.read_u8(2).unwrap() as u32) << 16;
            data |= (dma_bus.cdrom.read_u8(2).unwrap() as u32) << 24;

            dma_bus.main_ram.write_u32(address, data).unwrap();

            // step
            address += 4;
        }

        // only update the address and block control if chopping is enabled.
        // TODO: currently, we don't actually do chopping, the transfer
        //       will finish in one go regardless
        if chopping {
            channel.block_control = 0;
            channel.base_address = address;
        }

        // chrom transfer rate:
        // BIOS: 24 clk/word
        // GAMES: 40 clk/word
        //
        // Not sure exactly what is BIOS and what is GAMES, so for now, lets make
        // it 30 clk/word (around the middle).
        (block_size * 30, true)
    }

    fn perform_spu_channel4_dma(
        channel: &mut DmaChannel,
        dma_bus: &mut super::DmaBus,
    ) -> (u32, bool) {
        // must be sync mode 0 or 1
        assert!(channel.channel_control.sync_mode() != 2);
        // chopping is only for sync mode 0
        assert!(!channel
            .channel_control
            .intersects(ChannelControl::CHOPPING_ENABLED));

        let direction_from_main_ram = channel
            .channel_control
            .intersects(ChannelControl::DIRECTION_FROM_RAM);

        // check first that the SPU is ready for DMA transfer
        if !dma_bus.spu.is_ready_for_dma(direction_from_main_ram) {
            return (0, false);
        }

        let address_step = channel.channel_control.address_step();

        // TODO: check if the max is 16 or not
        let block_size = channel.block_control & 0xFFFF;
        let mut blocks = channel.block_control >> 16;

        let mut address = channel.base_address & 0xFFFFFC;

        if direction_from_main_ram {
            let mut block = Vec::with_capacity(block_size as usize);
            for _ in 0..block_size {
                let data = dma_bus.main_ram.read_u32(address).unwrap();
                block.push(data);
                // step
                address = (address as i32 + address_step) as u32;
            }

            dma_bus.spu.dma_write_buf(&block);
        } else {
            let block = dma_bus.spu.dma_read_buf(block_size as usize);

            for data in block {
                dma_bus.main_ram.write_u32(address, data).unwrap();
                // step
                address = (address as i32 + address_step) as u32;
            }
        }

        // sync mode 0, does everything in one go, and doesn't update the register
        // TODO: fix if we are in chopping
        if channel.channel_control.sync_mode() == 1 {
            blocks -= 1;

            channel.block_control &= 0xFFFF;
            channel.block_control |= blocks << 16;
            channel.base_address = address;
        }

        // we don't care about the block value in sync mode 0
        let finished = blocks == 0 || channel.channel_control.sync_mode() == 0;

        if finished {
            dma_bus.spu.finish_dma();
        }

        (block_size, finished)
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
            .intersects(ChannelControl::DIRECTION_FROM_RAM));
        // must be backwards
        assert!(channel.channel_control.address_step() == -4);
        // must be sync mode 0
        assert!(channel.channel_control.sync_mode() == 0);

        // must be triggered manually
        if !channel
            .channel_control
            .intersects(ChannelControl::START_TRIGGER)
        {
            return (0, true);
        }

        let chopping = channel
            .channel_control
            .intersects(ChannelControl::CHOPPING_ENABLED);

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
            dma_bus.main_ram.write_u32(current, next).unwrap();
            current = next;
        }
        dma_bus.main_ram.write_u32(current, 0xFFFFFF).unwrap();

        if chopping {
            channel.block_control = 0;
            channel.base_address = current;
        }

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

    /// Gets the channels to run based on priority, we are getting an array, so that if we couldn't
    /// run the most important channel (because its not ready yet), we can run the next one.
    fn get_channels_order_to_run<'a>(&self, out_order: &'a mut [usize]) -> &'a [usize] {
        // a location to store the enabled channels id and their priority
        // the initial value doesn't matter as it will be overwritten
        let mut enabled_channels = [(0, 0); 7];

        let mut total_enabled = 0;
        self.channels
            .iter()
            .enumerate()
            .filter_map(|(i, channel)| {
                let channel_enabled = (self.control >> (i * 4)) & 0b1000 != 0;
                let priority = (self.control >> (i * 4)) & 0b111;

                if channel_enabled && channel.channel_control.in_progress() {
                    Some((i, priority))
                } else {
                    None
                }
            })
            .for_each(|(i, priority)| {
                enabled_channels[total_enabled] = (i, priority);
                total_enabled += 1;
            });

        if total_enabled == 0 {
            return &[];
        }

        // sort by priority
        // high priority is 0, low priority is 7
        // if the priority is the same, the channel with the highest index is run first
        enabled_channels[..total_enabled]
            .sort_unstable_by_key(|(i, priority)| *priority as i32 * 100 - *i as i32);

        // copy the channels to run to the output array
        let size = out_order.len().min(total_enabled);
        for i in 0..size {
            out_order[i] = enabled_channels[i].0;
        }

        &out_order[..total_enabled]
    }

    pub(super) fn clock_dma(
        &mut self,
        dma_bus: &mut super::DmaBus,
        interrupt_requester: &mut impl InterruptRequester,
    ) -> u32 {
        // record the number of cycles that are spent of the cpu
        let mut cpu_cycles = 0;

        let mut channels_order = [0; 7];
        let channels_to_run = self.get_channels_order_to_run(&mut channels_order);
        for &i in channels_to_run {
            let channel = &mut self.channels[i];
            log::trace!("channel {} doing DMA", i);

            let (cycles_to_delay, finished) = match i {
                0 => Self::perform_mdec_in_channel0_dma(channel, dma_bus),
                1 => Self::perform_mdec_out_channel1_dma(channel, dma_bus),
                2 => Self::perform_gpu_channel2_dma(channel, dma_bus),
                3 => Self::perform_cdrom_channel3_dma(channel, dma_bus),
                4 => Self::perform_spu_channel4_dma(channel, dma_bus),
                5 => todo!("DMA channel PIO 5"),
                6 => Self::perform_otc_channel6_dma(channel, dma_bus),
                _ => unreachable!(),
            };

            if cycles_to_delay == 0 {
                continue;
            }

            cpu_cycles = cycles_to_delay;

            // remove trigger afterwards, since some handlers might check
            //  for manual trigger
            channel
                .channel_control
                .remove(ChannelControl::START_TRIGGER);

            if finished {
                channel.channel_control.finish_transfer();
                self.interrupt.request_interrupt(i as u32);
            }
            break;
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
    fn read_u32(&mut self, addr: u32) -> Result<u32> {
        let r = match addr {
            0x80..=0xEF => {
                let channel_index = (addr >> 4) - 8;
                log::info!("DMA, reading from channel {}", channel_index);
                self.channels[channel_index as usize].read(addr & 0xF)
            }
            0xF0 => self.control,
            0xF4 => self.interrupt.bits(),
            _ => unreachable!(),
        };
        Ok(r)
    }

    fn write_u32(&mut self, addr: u32, mut data: u32) -> Result<()> {
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
                let old_interrupt = self.interrupt.bits();
                let new_data = data & 0xFFFFFF;
                // and the flags will be reset on write
                let irq_flags_reset = data & 0x7F000000;

                // if a "channel enable" bit is disabled, clear the flag as well
                // read the enable and flip it, since 1 will reset the flag
                let irq_enable_mask = ((old_interrupt >> 16) & 0x7F) ^ 0x7F;
                let irq_flags_reset = irq_flags_reset | (irq_enable_mask << 24);

                let new_interrupt = ((old_interrupt & 0xFF000000) & !irq_flags_reset) | new_data;

                self.interrupt = DmaInterruptRegister::from_bits_retain(new_interrupt);
                log::info!(
                    "DMA interrupt input: {:08X}, result: {:08X}, {:?}",
                    data,
                    self.interrupt.bits(),
                    self.interrupt
                );
            }
            _ => unreachable!(),
        }

        Ok(())
    }

    fn read_u8(&mut self, addr: u32) -> Result<u8> {
        let u32_data = self.read_u32(addr & 0xFFFFFFFC)?;
        let shift = (addr & 3) * 8;

        Ok(((u32_data >> shift) & 0xFF) as u8)
    }

    fn write_u8(&mut self, addr: u32, data: u8) -> Result<()> {
        match addr {
            // most register, and interrupt flags
            0x80..=0xF3 | 0xF7 => {
                let aligned_addr = addr & 0xFFFFFFFC;
                let current_u32 = self.read_u32(aligned_addr)?;
                let shift = (addr & 3) * 8;
                let new_u32 = (current_u32 & !(0xFF << shift)) | ((data as u32) << shift);
                self.write_u32(aligned_addr, new_u32)?;
            }
            // the lower section of the interrrupt register
            // is special becasue we don't want to reset interrupts.
            0xF4..=0xF6 => {
                let current_u32 = self.read_u32(0xF4)?;
                let shift = (addr & 3) * 8;
                let new_u32 = (current_u32 & !(0xFF << shift)) | ((data as u32) << shift);

                // writing 1 to the interrupt flag will reset it
                // and we don't want that if there is already interrupts
                // so we convert them to 0
                let new_u32 = new_u32 & !0xFF000000;
                self.write_u32(0xF4, new_u32)?;
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_channels_order_no_channels_enabled() {
        let dma = Dma::default();

        let mut channels_order = [0; 7];
        let channels_order = dma.get_channels_order_to_run(&mut channels_order);
        assert_eq!(channels_order, &[]);
    }

    #[test]
    fn get_channels_order_one_channel_enabled() {
        let mut dma = Dma {
            control: 0b0000_0000_0000_0000_0000_0000_0000_1000, // enable channel 0 with priority 0
            ..Dma::default()
        };
        dma.channels[0].channel_control = ChannelControl::START_BUSY;
        let mut channels_order = [0; 7];
        let channels_order = dma.get_channels_order_to_run(&mut channels_order);
        assert_eq!(channels_order, &[0]);
    }

    #[test]
    fn get_channels_order_multiple_channels_enabled_same_priority() {
        let mut dma = Dma {
            control: 0b0000_0000_0000_0000_1000_1000_1000_1000, // enable channels 0, 1, 2, 3 with priority 0
            ..Dma::default()
        };
        dma.channels[0].channel_control = ChannelControl::START_BUSY;
        dma.channels[1].channel_control = ChannelControl::START_BUSY;
        dma.channels[2].channel_control = ChannelControl::START_BUSY;
        dma.channels[3].channel_control = ChannelControl::START_BUSY;
        let mut channels_order = [0; 7];
        let channels_order = dma.get_channels_order_to_run(&mut channels_order);
        assert_eq!(channels_order, &[3, 2, 1, 0]);
    }

    #[test]
    fn get_channels_order_multiple_channels_enabled_different_priority() {
        let mut dma = Dma {
            control: 0b0000_0000_0000_0000_1010_1001_0000_0000, // enable channels 2 and 3 with priority 1 and 2
            ..Dma::default()
        };
        dma.channels[2].channel_control = ChannelControl::START_BUSY;
        dma.channels[3].channel_control = ChannelControl::START_BUSY;
        let mut channels_order = [0; 7];
        let channels_order = dma.get_channels_order_to_run(&mut channels_order);
        assert_eq!(channels_order, &[2, 3]);
    }

    #[test]
    fn get_channels_order_all_channels_enabled_different_priority() {
        let mut dma = Dma {
            control: 0b1111_1110_1101_1100_1011_1010_1001_1000, // enable all channels with reverse ordering
            ..Dma::default()
        };
        for i in 0..7 {
            dma.channels[i].channel_control = ChannelControl::START_BUSY;
        }
        let mut channels_order = [0; 7];
        let channels_order = dma.get_channels_order_to_run(&mut channels_order);

        assert_eq!(channels_order, &[0, 1, 2, 3, 4, 5, 6]);
    }
}
