use std::collections::VecDeque;

use crate::memory::BusLine;

bitflags::bitflags! {
    #[derive(Default)]
    struct SpuControl: u16 {
        const CD_AUDIO_ENABLE         = 0b0000000000000001;
        const EXTERNAL_AUDIO_ENABLE   = 0b0000000000000010;
        const CD_AUDIO_REVERB         = 0b0000000000000100;
        const EXTERNAL_AUDIO_REVERB   = 0b0000000000001000;
        const SOUND_RAM_TRANSFER_MODE = 0b0000000000110000;
        const IRQ9_ENABLE             = 0b0000000001000000;
        const REVERB_MASTER_ENABLE    = 0b0000000010000000;
        const NOISE_FREQ_STEP         = 0b0000001100000000;
        const NOICE_FREQ_SHIFT        = 0b0011110000000000;
        const MUTE_SPU                = 0b0100000000000000;
        const SPU_ENABLE              = 0b1000000000000000;
    }
}

bitflags::bitflags! {
    #[derive(Default)]
    struct SpuStat: u16 {
        const CURRENT_SPU_MODE                 = 0b0000000000111111;
        const IRQ_FLAG                         = 0b0000000001000000;
        /// Data Transfer DMA Read/Write Request ;seems to be same as SPUCNT.Bit5 (?)
        /// looks like it is clearned when in manual mode
        const DATA_TRANSFER_USING_DMA          = 0b0000000010000000;
        const DATA_TRANSFER_DMA_READ_REQ       = 0b0000000100000000;
        const DATA_TRANSFER_DMA_WRITE_REQ      = 0b0000001000000000;
        const DATA_TRANSFER_BUSY_FLAG          = 0b0000010000000000;
        const WRITE_FIRST_SECOND_H_CAPTURE_BUF = 0b0000100000000000;
        // const UNUSED                        = 0b1111000000000000;
    }
}

#[repr(transparent)]
#[derive(Default)]
struct VoicesFlag {
    bits: u32,
}

impl VoicesFlag {
    fn get(&self, index: usize) -> bool {
        self.bits & (1 << index) != 0
    }

    fn set_all(&mut self, value: u32) {
        // only 24 bits are used
        self.bits = value & 0xFFFFFF;
    }

    fn get_all(&self) -> u32 {
        self.bits
    }
}

bitflags::bitflags! {
    #[derive(Default)]
    struct ADSRMode: u32 {
        const SUSTAIN_LEVEL                    = 0b00000000000000000000000000001111;
        // decay step is fixed (-8)
        // decay direction is fixed (decrease)
        // decay mode is fixed (exponential)
        const DECAY_SHIFT                      = 0b00000000000000000000000011110000;
        const ATTACK_STEP                      = 0b00000000000000000000001100000000;
        // attack direction is fixed (increase)
        const ATTACK_SHIFT                     = 0b00000000000000000111110000000000;
        const ATTACK_MODE                      = 0b00000000000000001000000000000000;
        // release step is fixed (-8)
        // release direction is fixed (decrease)
        const RELEASE_SHIFT                    = 0b00000000000111110000000000000000;
        const RELEASE_MODE                     = 0b00000000001000000000000000000000;
        const SUSTAIN_STEP                     = 0b00000000110000000000000000000000;
        const SUSTAIN_SHIFT                    = 0b00011111000000000000000000000000;
        const SUSTAIN_DIRECTION                = 0b01000000000000000000000000000000;
        const SUSTAIN_MODE                     = 0b10000000000000000000000000000000;
        // const UNUSED                        = 0b00100000000000000000000000000000;
    }
}

#[derive(Default, Clone, Copy)]
struct Voice {
    volume_left: u16,
    volume_right: u16,

    internal_current_vol_left: u16,
    internal_current_vol_right: u16,

    /// pitch
    adpcm_sample_rate: u16,
    adpcm_start_address: u16,

    /// internal register to keep track of the current adpcm address
    /// since `adpcm_start_address` is not updated
    adpcm_current_address: u32,

    // ADSR = Attack, Decay, Sustain, Release
    adsr_mode: ADSRMode,

    adsr_current_vol: u16,
    adsr_repeat_address: u16,
}

pub struct Spu {
    main_vol_left: u16,
    main_vol_right: u16,

    reverb_out_vol_left: u16,
    reverb_out_vol_right: u16,
    reverb_work_base: u16,

    cd_vol_left: u16,
    cd_vol_right: u16,

    external_vol_left: u16,
    external_vol_right: u16,

    current_vol_left: u16,
    current_vol_right: u16,

    sound_ram_data_transfer_control: u16,
    sound_ram_data_transfer_address: u16,
    internal_sound_ram_address: u32,
    sound_ram_irq_address: u16,

    data_fifo: VecDeque<u16>,

    control: SpuControl,
    stat: SpuStat,

    key_on_flag: VoicesFlag,
    key_off_flag: VoicesFlag,
    fm_channel_flag: VoicesFlag,
    noise_channel_mode_flag: VoicesFlag,
    reverb_channel_mode_flag: VoicesFlag,
    channel_enable_flag: VoicesFlag,

    voices: [Voice; 24],

    reverb_config: [u16; 0x20],

    spu_ram: Box<[u8; 0x80000]>,
}

impl Default for Spu {
    fn default() -> Self {
        Self {
            main_vol_left: 0,
            main_vol_right: 0,
            reverb_out_vol_left: 0,
            reverb_out_vol_right: 0,
            reverb_work_base: 0,
            cd_vol_left: 0,
            cd_vol_right: 0,
            external_vol_left: 0,
            external_vol_right: 0,
            current_vol_left: 0,
            current_vol_right: 0,
            sound_ram_data_transfer_control: 0,
            sound_ram_data_transfer_address: 0,
            internal_sound_ram_address: 0,
            sound_ram_irq_address: 0,
            data_fifo: VecDeque::new(),
            control: SpuControl::default(),
            stat: SpuStat::default(),
            key_on_flag: VoicesFlag::default(),
            key_off_flag: VoicesFlag::default(),
            fm_channel_flag: VoicesFlag::default(),
            noise_channel_mode_flag: VoicesFlag::default(),
            reverb_channel_mode_flag: VoicesFlag::default(),
            channel_enable_flag: VoicesFlag::default(),
            voices: [Voice::default(); 24],
            reverb_config: [0; 0x20],
            spu_ram: Box::new([0; 0x80000]),
        }
    }
}

impl BusLine for Spu {
    fn read_u32(&mut self, addr: u32) -> u32 {
        match addr {
            0x000..=0x17F => todo!("u32 read voice register {:03X}", addr),
            0x180..=0x187 => todo!("u32 read spu control {:03X}", addr),
            0x188..=0x19F => todo!("u32 read voice flags {:03X}", addr),
            0x1A0..=0x1BF => todo!("u32 read spu  control {:03X}", addr),
            0x1C0..=0x1FF => todo!("u32 read reverb configuration {:03X}", addr),
            0x200..=0x25F => todo!("u32 read voice internal reg {:03X}", addr),
            0x260..=0x2FF => unreachable!("u32 read unknown {:03X}", addr),
            _ => unreachable!(),
        }
    }

    fn write_u32(&mut self, addr: u32, _data: u32) {
        match addr {
            0x000..=0x17F => todo!("u32 write voice register {:03X}", addr),
            0x180..=0x187 => todo!("u32 write spu control {:03X}", addr),
            0x188..=0x19F => todo!("u32 write voice flags {:03X}", addr),
            0x1A0..=0x1BF => todo!("u32 write spu  control {:03X}", addr),
            0x1C0..=0x1FF => todo!("u32 write reverb configuration {:03X}", addr),
            0x200..=0x25F => todo!("u32 write voice internal reg {:03X}", addr),
            0x260..=0x2FF => unreachable!("u32 write unknown {:03X}", addr),
            _ => unreachable!(),
        }
    }

    fn read_u16(&mut self, addr: u32) -> u16 {
        match addr {
            0x000..=0x17E => {
                let reg = addr & 0xF;
                let voice_idx = (addr >> 4) as usize;
                match reg {
                    0x0 => self.voices[voice_idx].volume_left,
                    0x2 => self.voices[voice_idx].volume_right,
                    0x4 => self.voices[voice_idx].adpcm_sample_rate,
                    0x6 => self.voices[voice_idx].adpcm_start_address,
                    0x8 => self.voices[voice_idx].adsr_mode.bits() as u16,
                    0xA => (self.voices[voice_idx].adsr_mode.bits() >> 16) as u16,
                    0xC => self.voices[voice_idx].adsr_current_vol,
                    0xE => self.voices[voice_idx].adsr_repeat_address,
                    _ => unreachable!(),
                }
            }
            0x180 => self.main_vol_left,
            0x182 => self.main_vol_right,
            0x184 => self.reverb_out_vol_left,
            0x186 => self.reverb_out_vol_right,
            // key on and key off should be treated as write only, reading
            // will only return the last written value
            0x188 => self.key_on_flag.get_all() as u16,
            0x18A => (self.key_on_flag.get_all() >> 16) as u16,
            0x18C => self.key_off_flag.get_all() as u16,
            0x18E => (self.key_off_flag.get_all() >> 16) as u16,
            0x190 => self.fm_channel_flag.get_all() as u16,
            0x192 => (self.fm_channel_flag.get_all() >> 16) as u16,
            0x194 => self.noise_channel_mode_flag.get_all() as u16,
            0x196 => (self.noise_channel_mode_flag.get_all() >> 16) as u16,
            0x198 => self.reverb_channel_mode_flag.get_all() as u16,
            0x19A => (self.reverb_channel_mode_flag.get_all() >> 16) as u16,
            0x19C => self.channel_enable_flag.get_all() as u16,
            0x19E => (self.channel_enable_flag.get_all() >> 16) as u16,
            0x180..=0x187 => todo!("u16 read spu control {:03X}", addr),
            0x188..=0x19F => todo!("u16 read voice flags {:03X}", addr),
            0x1A0 => unreachable!("u16 read unknown {:03X}", addr),
            0x1A2 => self.reverb_work_base,
            0x1A4 => self.sound_ram_irq_address,
            0x1A6 => self.sound_ram_data_transfer_address,
            0x1AA => self.control.bits(),
            0x1AC => self.sound_ram_data_transfer_control << 1,
            0x1AE => self.stat.bits(),
            0x1B0 => self.cd_vol_left,
            0x1B2 => self.cd_vol_right,
            0x1B4 => self.external_vol_left,
            0x1B6 => self.external_vol_right,
            0x1B8 => self.current_vol_left,
            0x1BA => self.current_vol_right,
            0x1A2..=0x1BF => todo!("u16 read spu  control {:03X}", addr),
            0x1C0..=0x1FE => self.reverb_config[(addr - 0x1C0) as usize / 2],
            //0x1C0..=0x1FF => todo!("u16 read reverb configuration {:03X}", addr),
            0x200..=0x25E => {
                let voice_idx = (addr >> 2) as usize;
                if addr & 0x2 == 0 {
                    self.voices[voice_idx].internal_current_vol_left
                } else {
                    self.voices[voice_idx].internal_current_vol_right
                }
            }
            0x260..=0x2FF => unreachable!("u16 read unknown {:03X}", addr),
            _ => unreachable!(),
        }
    }

    fn write_u16(&mut self, addr: u32, data: u16) {
        match addr {
            0x000..=0x17E => {
                let reg = addr & 0xF;
                let voice_idx = (addr >> 4) as usize;
                log::info!(
                    "u16 write voice {}, reg {:01X} = {:04X}",
                    voice_idx,
                    reg,
                    data
                );
                match reg {
                    0x0 => self.voices[voice_idx].volume_left = data,
                    0x2 => self.voices[voice_idx].volume_right = data,
                    0x4 => self.voices[voice_idx].adpcm_sample_rate = data,
                    0x6 => self.voices[voice_idx].adpcm_start_address = data,
                    0x8 => {
                        let f = self.voices[voice_idx].adsr_mode.bits();
                        self.voices[voice_idx].adsr_mode =
                            ADSRMode::from_bits_truncate((f & 0xFFFF0000) | data as u32);
                    }
                    0xA => {
                        let f = self.voices[voice_idx].adsr_mode.bits();
                        self.voices[voice_idx].adsr_mode =
                            ADSRMode::from_bits_truncate((f & 0xFFFF) | ((data as u32) << 16));
                    }
                    0xC => self.voices[voice_idx].adsr_current_vol = data,
                    0xE => self.voices[voice_idx].adsr_repeat_address = data,
                    _ => unreachable!(),
                }
            }
            0x180 => self.main_vol_left = data,
            0x182 => self.main_vol_right = data,
            0x184 => self.reverb_out_vol_left = data,
            0x186 => self.reverb_out_vol_right = data,
            0x188 => {
                let f = self.key_on_flag.get_all();
                self.key_on_flag.set_all((f & 0xFFFF0000) | data as u32);
            }
            0x18A => {
                let f = self.key_on_flag.get_all();
                self.key_on_flag
                    .set_all((f & 0x0000FFFF) | ((data as u32) << 16));
            }
            0x18C => {
                let f = self.key_off_flag.get_all();
                self.key_off_flag.set_all((f & 0xFFFF0000) | data as u32);
            }
            0x18E => {
                let f = self.key_off_flag.get_all();
                self.key_off_flag
                    .set_all((f & 0x0000FFFF) | ((data as u32) << 16));
            }
            0x190 => {
                let f = self.fm_channel_flag.get_all();
                self.fm_channel_flag.set_all((f & 0xFFFF0000) | data as u32);
            }
            0x192 => {
                let f = self.fm_channel_flag.get_all();
                self.fm_channel_flag
                    .set_all((f & 0x0000FFFF) | ((data as u32) << 16));
            }
            0x194 => {
                let f = self.noise_channel_mode_flag.get_all();
                self.noise_channel_mode_flag
                    .set_all((f & 0xFFFF0000) | data as u32);
            }
            0x196 => {
                let f = self.noise_channel_mode_flag.get_all();
                self.noise_channel_mode_flag
                    .set_all((f & 0x0000FFFF) | ((data as u32) << 16));
            }
            0x198 => {
                let f = self.reverb_channel_mode_flag.get_all();
                self.reverb_channel_mode_flag
                    .set_all((f & 0xFFFF0000) | data as u32);
            }
            0x19A => {
                let f = self.reverb_channel_mode_flag.get_all();
                self.reverb_channel_mode_flag
                    .set_all((f & 0x0000FFFF) | ((data as u32) << 16));
            }
            // channel enable should be read only
            // writing to it will save the data, but it doesn't have any affect
            // on the hardware functionality, and will be overwritten by the hardware
            0x19C => {
                let f = self.channel_enable_flag.get_all();
                self.channel_enable_flag
                    .set_all((f & 0xFFFF0000) | data as u32);
            }
            0x19E => {
                let f = self.channel_enable_flag.get_all();
                self.channel_enable_flag
                    .set_all((f & 0x0000FFFF) | ((data as u32) << 16));
            }
            0x1A0 => unreachable!("u16 write unknown {:03X}", addr),
            0x1A2 => self.reverb_work_base = data,
            0x1A4 => self.sound_ram_irq_address = data,
            0x1A6 => {
                log::info!("sound ram data transfer address {:04X}", data);
                self.sound_ram_data_transfer_address = data;
                self.internal_sound_ram_address = data as u32 * 8;
            }
            0x1A8 => {
                log::info!("sound ram data transfer fifo {:04X}", data);
                if self.data_fifo.len() == 32 {
                    panic!("sound ram data transfer fifo overflow");
                }
                //self.data_fifo.push_back(data);
            }
            0x1AA => {
                // TODO: handle ack
                self.control = SpuControl::from_bits_truncate(data);
                log::info!("spu control {:04X}", data);

                // the lower 6 bits of the stat reg are the control reg
                // in reality, it is applied after a delay, but we don't need
                // to emualte it.
                self.stat.remove(SpuStat::CURRENT_SPU_MODE);
                self.stat |= SpuStat::from_bits_truncate(data & 0x3F);
            }
            0x1AC => self.sound_ram_data_transfer_control = (data >> 1) & 7,
            0x1AE => unreachable!("u16 write SpuStat is not supported"),
            0x1B0 => self.cd_vol_left = data,
            0x1B2 => self.cd_vol_right = data,
            0x1B4 => self.external_vol_left = data,
            0x1B6 => self.external_vol_right = data,
            0x1B8 | 0x1BA => unreachable!("u16 write current volume is not supported {:03X}", addr),
            0x1A2..=0x1BF => todo!("u16 write spu  control {:03X}", addr),
            0x1C0..=0x1FE => self.reverb_config[(addr - 0x1C0) as usize / 2] = data,
            //0x1C0..=0x1FF => todo!("u16 write reverb configuration {:03X}", addr),
            // TODO: not sure if this is writable, since its internal current vol
            0x200..=0x25F => todo!("u16 write voice internal reg {:03X}", addr),
            0x260..=0x2FF => unreachable!("u16 write unknown {:03X}", addr),
            _ => unreachable!(),
        }
    }

    fn read_u8(&mut self, _addr: u32) -> u8 {
        todo!()
    }

    fn write_u8(&mut self, addr: u32, _data: u8) {
        // The SPU is connected to a 16bit databus.
        // 8bit/16bit/32bit reads and 16bit/32bit writes are implemented.
        // However, 8bit writes are NOT implemented: 8bit writes to
        // ODD addresses are simply ignored (without causing any exceptions),
        // 8bit writes to EVEN addresses are executed as 16bit writes
        // (eg. "movp r1,12345678h, movb [spu_port],r1" will write 5678h instead of 78h).
        //
        // TODO: implement this behavior of 8bit writes, we need to get access
        //       to the whole 32bit word from the CPU

        todo!("spu corrupt 8bit write addr: {:03X}", addr);
    }
}
