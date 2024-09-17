use std::{
    cell::Cell,
    collections::VecDeque,
    ops::{Index, IndexMut, Range},
};

use crate::memory::{interrupts::InterruptRequester, BusLine, Result};

const CPU_CLOCKS_PER_SPU: u32 = 0x300;

enum RamTransferMode {
    Stop,
    ManualWrite,
    DmaWrite,
    DmaRead,
}

bitflags::bitflags! {
    #[derive(Default, Debug)]
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
        const UNMUTE_SPU              = 0b0100000000000000;
        const SPU_ENABLE              = 0b1000000000000000;
    }
}

impl SpuControl {
    fn ram_transfer_mode(&self) -> RamTransferMode {
        match self.bits() & Self::SOUND_RAM_TRANSFER_MODE.bits() {
            0b000000 => RamTransferMode::Stop,
            0b010000 => RamTransferMode::ManualWrite,
            0b100000 => RamTransferMode::DmaWrite,
            0b110000 => RamTransferMode::DmaRead,
            _ => unreachable!(),
        }
    }
}

bitflags::bitflags! {
    #[derive(Default, Debug)]
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

const ADPCM_TABLE_POS: &[i32; 5] = &[0, 60, 115, 98, 122];
const ADPCM_TABLE_NEG: &[i32; 5] = &[0, 0, -52, -55, -60];

#[derive(Default, Clone, Copy)]
struct AdpcmDecoder {
    old: i32,
    older: i32,
}

impl AdpcmDecoder {
    /// Decode ADPCM block
    ///
    /// `in_block` must be 16 bytes long (8 elements)
    fn decode_block(&mut self, in_block: &[u16], out: &mut [i16; 28]) {
        assert_eq!(in_block.len(), 8);

        let shift_filter = in_block[0] & 0xFF;

        // adpcm decoding...
        // 13, 14, 15 are reserved and behave similar to 9
        let shift_factor = 12u16.checked_sub(shift_filter & 0xf).unwrap_or(12 - 9);
        let filter = shift_filter >> 4;

        // only 5 filters supported in SPU ADPCM
        // for some reason, some games audio will be outside the range 0-4
        let filter = filter % 5;

        let f0 = ADPCM_TABLE_POS[filter as usize];
        let f1 = ADPCM_TABLE_NEG[filter as usize];

        for i in 0..28 / 4 {
            // 4 samples together
            let mut adpcm_16bit_chunk = in_block[i + 1];
            for j in 0..4 {
                let adpcm_4bit_sample = adpcm_16bit_chunk & 0xF;
                let mut sample = adpcm_4bit_sample as i32;
                // convert to signed 32 from 4 bit
                if sample & 0x8 != 0 {
                    sample = ((sample as u32) | 0xfffffff0) as i32;
                }
                // shift
                sample <<= shift_factor;
                // apply adpcm filter
                sample += (self.old * f0 + self.older * f1 + 32) / 64;
                sample = sample.clamp(-0x8000, 0x7fff);

                self.older = self.old;
                self.old = sample;

                out[i * 4 + j] = sample as i16;

                // next nibble
                adpcm_16bit_chunk >>= 4;
            }
        }
    }
}

#[derive(Default)]
struct VoicesFlag {
    bits: u32,
}

impl VoicesFlag {
    fn get(&self, index: usize) -> bool {
        self.bits & (1 << index) != 0
    }

    fn set(&mut self, index: usize, value: bool) {
        self.bits &= !(1 << index);
        self.bits |= (value as u32) << index;
    }

    fn bus_set_all(&mut self, value: u32) {
        // only 24 bits are used
        self.bits = value & 0xFFFFFF;
    }

    fn get_all(&self) -> u32 {
        self.bits
    }
}

bitflags::bitflags! {
    #[derive(Default, Clone, Copy)]
    struct ADSRConfig: u32 {
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

impl std::fmt::Debug for ADSRConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const STEPS_POS: &[i16; 4] = &[7, 6, 5, 4];
        const STEPS_NEG: &[i16; 4] = &[-8, -7, -6, -5];

        let attack_mode_exp = self.contains(ADSRConfig::ATTACK_MODE);
        let attack_shift = ((self.bits() >> 10) & 0b11111) as u8;
        let step_i = (self.bits() >> 8) & 0b11;
        let attack_step = STEPS_POS[step_i as usize];

        let decay_shift = ((self.bits() >> 4) & 0b1111) as u8;

        let sustain_level_mul = (self.bits() & 0b1111) as u16 + 1;
        let sustain_level = (sustain_level_mul * 0x800).max(0x7FFF);

        let sustain_mode_exp = self.contains(ADSRConfig::SUSTAIN_MODE);
        let sustain_direction_dec = self.contains(ADSRConfig::SUSTAIN_DIRECTION);
        let sustain_shift = ((self.bits() >> 24) & 0b11111) as u8;

        let step_i = (self.bits() >> 22) & 0b11;
        let sustain_step = if sustain_direction_dec {
            STEPS_NEG[step_i as usize]
        } else {
            STEPS_POS[step_i as usize]
        };
        let release_mode_exp = self.contains(ADSRConfig::RELEASE_MODE);
        let release_shift = ((self.bits() >> 16) & 0b11111) as u8;

        f.debug_struct("ADSRConfig")
            .field("attack_mode_exp", &attack_mode_exp)
            .field("attack_shift", &attack_shift)
            .field("attack_step", &attack_step)
            .field("decay_shift", &decay_shift)
            .field("sustain_level", &sustain_level)
            .field("sustain_mode_exp", &sustain_mode_exp)
            .field("sustain_direction_dec", &sustain_direction_dec)
            .field("sustain_shift", &sustain_shift)
            .field("sustain_step", &sustain_step)
            .field("release_mode_exp", &release_mode_exp)
            .field("release_shift", &release_shift)
            .finish()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum ADSRState {
    Attack,
    Decay,
    Sustain,
    Release,
    #[default]
    Stopped,
}

#[derive(Default, Clone, Copy)]
struct Voice {
    volume_left: u16,
    volume_right: u16,

    current_vol_left: i16,
    current_vol_right: i16,

    /// pitch
    adpcm_sample_rate: u16,
    /// `i_` here and in many places means `internal`,
    /// which is not part of the SPU external API (Bus access)
    ///
    /// The current volume is used for sweep envelope
    i_adpcm_pitch_counter: u32,

    adpcm_start_address: u16,
    adpcm_repeat_address: u16,
    /// internal register to keep track of the current adpcm address
    /// since `adpcm_start_address` is not updated
    i_adpcm_current_address: usize,

    i_adpcm_decoder: AdpcmDecoder,

    adsr_config: ADSRConfig,

    i_adsr_state: ADSRState,

    /// This is also refer to `ADSRLevel`
    ///
    /// DOCS: The register is read/writeable, writing allows to let the
    /// ADSR generator to "jump" to a specific volume level. But, ACTUALLY,
    /// the ADSR generator does overwrite the setting (from another internal
    /// register) whenever applying a new Step?!
    ///
    /// FIXME: currently, we are directly using this, we should use internal register
    adsr_current_vol: u16,

    i_cached_28_samples_block: [i16; 28],
    // 0 means that there is no cached block, so we must fetch,decode,cache it
    // and use the first sample
    i_cached_sample_index: usize,

    i_adsr_cycle_counter: u32,

    is_on: bool,
    is_off: bool,
}

impl Voice {
    fn key_on(&mut self) {
        self.i_adpcm_current_address = self.adpcm_start_address as usize * 4;
        self.i_cached_sample_index = 28;
        self.i_adpcm_decoder.old = 0;
        self.i_adpcm_decoder.older = 0;
        self.adpcm_repeat_address = self.adpcm_start_address;
        self.set_adsr_state(ADSRState::Attack);
        self.adsr_current_vol = 0;
        self.is_on = true;
        self.is_off = false;
    }

    fn key_off(&mut self) {
        self.set_adsr_state(ADSRState::Release);
        self.is_off = true;
    }

    fn set_adsr_state(&mut self, state: ADSRState) {
        self.i_adsr_state = state;
        self.i_adsr_cycle_counter = 0;
    }

    /// returns
    /// - `mode`: 0=linear, 1=exponential
    /// - `direction`: 0=increase, 1=decrease
    /// - `shift`
    /// - `step`
    /// - `target_level`
    fn get_adsr_current_info(&self) -> (bool, bool, u8, i16, u16) {
        const STEPS_POS: &[i16; 4] = &[7, 6, 5, 4];
        const STEPS_NEG: &[i16; 4] = &[-8, -7, -6, -5];

        let mode;
        let direction;
        let shift;
        let step;
        let target_level;
        match self.i_adsr_state {
            ADSRState::Attack => {
                mode = self.adsr_config.contains(ADSRConfig::ATTACK_MODE);
                direction = false; // always increase
                shift = ((self.adsr_config.bits() >> 10) & 0b11111) as u8;
                let step_i = (self.adsr_config.bits() >> 8) & 0b11;
                step = STEPS_POS[step_i as usize];
                target_level = 0x7FFF;
            }
            ADSRState::Decay => {
                mode = true; // always exponential
                direction = true; // always decrease
                shift = ((self.adsr_config.bits() >> 4) & 0b1111) as u8;
                step = -8; // always -8

                // until sustain level
                let sustain_level_mul = (self.adsr_config.bits() & 0b1111) as u16 + 1;
                // the max level will be 0x8000, which is outside the range
                // so clamp it.
                target_level = (sustain_level_mul * 0x800).max(0x7FFF);
            }
            ADSRState::Sustain => {
                mode = self.adsr_config.contains(ADSRConfig::SUSTAIN_MODE);
                direction = self.adsr_config.contains(ADSRConfig::SUSTAIN_DIRECTION);
                shift = ((self.adsr_config.bits() >> 24) & 0b11111) as u8;
                let step_i = (self.adsr_config.bits() >> 22) & 0b11;
                step = if direction {
                    STEPS_NEG[step_i as usize]
                } else {
                    STEPS_POS[step_i as usize]
                };
                target_level = 0; // not important, there is no target level
                                  // will not switch until Key off
            }
            ADSRState::Release => {
                mode = self.adsr_config.contains(ADSRConfig::RELEASE_MODE);
                direction = true; // always decrease
                shift = ((self.adsr_config.bits() >> 16) & 0b11111) as u8;
                step = -8; // always -8
                target_level = 0; // until 0
            }
            ADSRState::Stopped => {
                mode = false;
                direction = true;
                shift = 0;
                step = 0;
                target_level = 0;
            }
        }

        (mode, direction, shift, step, target_level)
    }

    fn clock_adsr(&mut self) {
        // ADSR operation
        //
        //  AdsrCycles = 1 SHL Max(0,ShiftValue-11)
        //  AdsrStep = StepValue SHL Max(0,11-ShiftValue)
        //  IF exponential AND increase AND AdsrLevel>6000h THEN AdsrCycles=AdsrCycles*4
        //  IF exponential AND decrease THEN AdsrStep=AdsrStep*AdsrLevel/8000h
        //  Wait(AdsrCycles)              ;cycles counted at 44.1kHz clock
        //  AdsrLevel=AdsrLevel+AdsrStep  ;saturated to 0..+7FFFh
        // FIXME: we are waiting first, then adding the step together with the
        //        rest, not sure if this is correct, but for now its simpler
        if self.i_adsr_cycle_counter > 0 {
            self.i_adsr_cycle_counter -= 1;
            return;
        }

        let (mode_exponential, direction_decrease, shift, step, target_level) =
            self.get_adsr_current_info();

        if self.i_adsr_state == ADSRState::Stopped {
            self.is_on = false;
            return;
        }

        let mut adsr_cycles = 1 << shift.saturating_sub(11);
        let mut adsr_step = step << (11u8).saturating_sub(shift);

        // fake exponential
        if mode_exponential {
            if direction_decrease {
                adsr_step = (adsr_step as i32 * self.adsr_current_vol as i32 / 0x8000)
                    .clamp(-0x8000, 0x7FFF) as i16;
                if adsr_step == 0 {
                    adsr_step = -1;
                }
            } else if self.adsr_current_vol > 0x6000 {
                if shift < 10 {
                    adsr_step /= 4;
                } else if shift >= 11 {
                    adsr_cycles *= 4;
                } else {
                    adsr_step /= 4;
                    adsr_cycles *= 4;
                }
            }
        }

        self.i_adsr_cycle_counter = adsr_cycles.max(1);

        // should wait here
        self.adsr_current_vol =
            ((self.adsr_current_vol as i16).saturating_add(adsr_step)).clamp(0, 0x7FFF) as u16;

        if (direction_decrease && self.adsr_current_vol <= target_level)
            || (!direction_decrease && self.adsr_current_vol >= target_level)
        {
            match self.i_adsr_state {
                ADSRState::Attack => {
                    self.i_adsr_state = ADSRState::Decay;
                }
                ADSRState::Decay => {
                    self.i_adsr_state = ADSRState::Sustain;
                }
                ADSRState::Sustain => {
                    // do nothing
                }
                ADSRState::Release => {
                    self.i_adsr_state = ADSRState::Stopped;
                }
                ADSRState::Stopped => {
                    // do nothing
                }
            }
        }
    }

    /// returns `true` if `ENDX` should be set
    ///
    /// This may set ADSR mode to `Release` when encountering the flags `End+Mute`
    fn fetch_and_decode_next_sample_block(&mut self, ram: &SpuRam) -> bool {
        let mut endx_set = false;

        // 16 bytes block
        let adpcm_block = &ram[self.i_adpcm_current_address..self.i_adpcm_current_address + 8];
        // move to next block
        self.i_adpcm_current_address += 8;
        self.i_adpcm_current_address &= 0x3FFFF;

        let flags = adpcm_block[0] >> 8;

        // handle flags/looping/etc.
        let loop_end = flags & 1 == 1;
        let loop_repeat = flags & 2 == 2;
        let loop_start = flags & 4 == 4;

        if loop_start {
            // make sure we use the `current` value, not the `next` value
            self.adpcm_repeat_address = ((self.i_adpcm_current_address - 8) / 4) as u16;
        }
        if loop_end {
            self.i_adpcm_current_address = self.adpcm_repeat_address as usize * 4;
            endx_set = true;
            if loop_repeat {
                // `End+Repeat`: jump to Loop-address, set ENDX flag
            } else {
                // `End+Mute`: jump to Loop-address, set ENDX flag, Release, Env=0000h
                self.set_adsr_state(ADSRState::Release);
                // clear the adsr envelope
                self.adsr_config = ADSRConfig::empty();
            }
        }

        self.i_adpcm_decoder
            .decode_block(adpcm_block, &mut self.i_cached_28_samples_block);

        endx_set
    }

    /// returns
    /// - `true` if `ENDX` should be set
    /// - `mono_output` can be used for capture
    /// - `left_output`
    /// - `right_output`
    fn clock_voice(&mut self, ram: &SpuRam) -> (bool, i16, i32, i32) {
        self.clock_adsr();

        let mut endx_set = false;

        if self.i_cached_sample_index >= 28 {
            endx_set = self.fetch_and_decode_next_sample_block(ram);

            // keep the bottom to not disturb the pitch modulation
            self.i_adpcm_pitch_counter &= 0x3FFF;
            self.i_cached_sample_index = 0;
        }

        let current_index = self.i_cached_sample_index;

        let mut step = self.adpcm_sample_rate;

        // clamp
        if step > 0x3FFF {
            step = 0x4000;
        }

        // handle sample rate
        self.i_adpcm_pitch_counter += step as u32;
        // Counter.Bit12 and up indicates the current sample (within a ADPCM block).
        let next_sample = self.i_adpcm_pitch_counter >> 12;
        // TODO: add pitch modulation
        // Counter.Bit3..11 are used as 8bit gaussian interpolation index

        self.i_cached_sample_index = next_sample as usize;

        let current_sample = self.i_cached_28_samples_block[current_index];

        // This `mono output` can be used in the capture buffer, the remaining
        // volume control and sweep are not included in the capture buffer data.
        let mono_output =
            (current_sample as i32 * self.adsr_current_vol as i32 / 0x8000).clamp(-0x8000, 0x7FFF);

        // TODO: implement sweep
        #[allow(unused)]
        let sweep_mode = self.volume_left & 0x8000 == 0x8000;

        let left_output =
            (mono_output * self.current_vol_left as i32 / 0x8000).clamp(-0x8000, 0x7FFF);
        let right_output =
            (mono_output * self.current_vol_right as i32 / 0x8000).clamp(-0x8000, 0x7FFF);

        (endx_set, mono_output as i16, left_output, right_output)
    }
}

// 1KB of RAM (16bit)
const CAPTURE_MEMORY_REGION_SIZE: usize = 0x200;

struct SpuRam {
    data: Box<[u16; 0x40000]>,
    /// The address from the ram, when read/written to it should trigger interrupt
    irq_address: usize,
    /// will store whether the IRQ was triggered
    /// Handling and signaling interrupt to the other hardware is done by the `Spu` itself.
    /// must be cleared by the handler before the next IRQ can be triggered
    irq_flag: Cell<bool>,

    /// The saved location of the pointer to store the next sample for the cd left audio
    /// in the ram (this goes from 0 to 0x1FF)
    cd_left_capture_index: usize,

    /// The saved location of the pointer to store the next sample for the cd right audio
    /// in the ram (this goes from 0 to 0x1FF)
    cd_right_capture_index: usize,

    /// The saved location of the pointer to store the next sample for the voice 1 mono audio
    /// in the ram (this goes from 0 to 0x1FF)
    voice_1_mono_capture_index: usize,

    /// The saved location of the pointer to store the next sample for the voice 3 mono audio
    /// in the ram (this goes from 0 to 0x1FF)
    voice_3_mono_capture_index: usize,
}

impl SpuRam {
    pub fn reset_irq(&self) {
        self.irq_flag.set(false);
    }

    pub fn push_cd_capture_samples(&mut self, left: i16, right: i16) {
        {
            let i = self.cd_left_capture_index;
            self[i] = left as u16;
        }
        {
            let i = 0x200 + self.cd_right_capture_index;
            // offset by 1KB
            self[i] = right as u16;
        }

        self.cd_left_capture_index = (self.cd_left_capture_index + 1) % CAPTURE_MEMORY_REGION_SIZE;
        self.cd_right_capture_index =
            (self.cd_right_capture_index + 1) % CAPTURE_MEMORY_REGION_SIZE;
    }

    pub fn push_voice_1_sample(&mut self, sample: i16) {
        let i = 0x400 + self.voice_1_mono_capture_index;
        self[i] = sample as u16;
        self.voice_1_mono_capture_index =
            (self.voice_1_mono_capture_index + 1) % CAPTURE_MEMORY_REGION_SIZE;
    }

    pub fn push_voice_3_sample(&mut self, sample: i16) {
        let i = 0x600 + self.voice_3_mono_capture_index;
        self[i] = sample as u16;
        self.voice_3_mono_capture_index =
            (self.voice_3_mono_capture_index + 1) % CAPTURE_MEMORY_REGION_SIZE;
    }
}

impl Index<Range<usize>> for SpuRam {
    type Output = [u16];

    fn index(&self, index: Range<usize>) -> &Self::Output {
        if index.contains(&self.irq_address) {
            self.irq_flag.set(true);
        }
        self.data.index(index)
    }
}

impl Index<usize> for SpuRam {
    type Output = u16;

    fn index(&self, index: usize) -> &Self::Output {
        if index == self.irq_address {
            self.irq_flag.set(true);
        }
        &self.data[index]
    }
}

impl IndexMut<usize> for SpuRam {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        if index == self.irq_address {
            self.irq_flag.set(true);
        }
        &mut self.data[index]
    }
}

impl Default for SpuRam {
    fn default() -> Self {
        Self {
            data: Box::new([0; 0x40000]),
            irq_address: 0x0,
            irq_flag: Cell::new(false),
            cd_left_capture_index: 0,
            cd_right_capture_index: 0,
            voice_1_mono_capture_index: 0,
            voice_3_mono_capture_index: 0,
        }
    }
}

#[derive(Default)]
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

    current_main_vol_left: i16,
    current_main_vol_right: i16,

    ram_transfer_control: u16,
    ram_transfer_address: u16,
    i_ram_transfer_address: usize,

    write_data_fifo: VecDeque<u16>,

    control: SpuControl,
    stat: SpuStat,

    key_on_flag: VoicesFlag,
    key_off_flag: VoicesFlag,
    // should the voice be pitch modulated
    pitch_mod_channel_flag: VoicesFlag,
    // should the voice be in noise mode or not
    noise_channel_mode_flag: VoicesFlag,
    // should the voice be used in reverb
    reverb_channel_mode_flag: VoicesFlag,
    // updated by the SPU
    // The bits get CLEARED when setting the corresponding KEY ON bits.
    // The bits get SET when reaching an LOOP-END flag in ADPCM header.bit0.
    endx_flag: VoicesFlag,

    voices: [Voice; 24],

    reverb_config: [u16; 0x20],

    spu_ram: SpuRam,

    cdrom_audio_buffer_left: VecDeque<i16>,
    cdrom_audio_buffer_right: VecDeque<i16>,

    /// internal timer to know when to run the SPU.
    /// The SPU runs at 44100Hz, which is CPU_CLOCK / 0x300
    /// I guess the CPU clock was designed around the SPU?
    cpu_clock_timer: u32,

    /// Output audio stereo in 44100Hz 16PCM
    out_audio_buffer: Vec<f32>,

    in_dma_transfer: bool,
}

impl Spu {
    pub fn clock(&mut self, interrupt_requester: &mut impl InterruptRequester, cycles: u32) {
        self.cpu_clock_timer += cycles;

        loop {
            if self.cpu_clock_timer < CPU_CLOCKS_PER_SPU {
                break;
            }
            self.cpu_clock_timer -= CPU_CLOCKS_PER_SPU;

            // clear internal irq flag
            self.spu_ram.reset_irq();

            // the order of SPU handling is
            // - voice1
            // - write cd left
            // - write cd right
            // - write voice 1 to capture
            // - write voice 3 to capture
            // - voice2..24
            // Maybe it is starting from voice2? so that writing will be at the end?
            //
            // anyway, the order doesn't matter since there is no dependancy between them
            // except that each voice depend on the previous one in pitch modulation

            // reflect the spu stat
            self.stat.remove(SpuStat::CURRENT_SPU_MODE);
            self.stat |= SpuStat::from_bits_retain(self.control.bits() & 0x3F);

            // handle memory transfers
            match self.control.ram_transfer_mode() {
                RamTransferMode::Stop => {
                    self.stat.remove(
                        SpuStat::DATA_TRANSFER_BUSY_FLAG
                            | SpuStat::DATA_TRANSFER_USING_DMA
                            | SpuStat::DATA_TRANSFER_DMA_WRITE_REQ
                            | SpuStat::DATA_TRANSFER_DMA_READ_REQ,
                    );
                }
                RamTransferMode::ManualWrite => {
                    if self.write_data_fifo.is_empty() {
                        self.stat.remove(SpuStat::DATA_TRANSFER_BUSY_FLAG);
                    } else {
                        self.stat.insert(SpuStat::DATA_TRANSFER_BUSY_FLAG);

                        for d in self.write_data_fifo.drain(..) {
                            self.spu_ram[self.i_ram_transfer_address] = d;
                            self.i_ram_transfer_address += 1;
                            self.i_ram_transfer_address &= 0x3FFFF
                        }
                        // reset the busy flag on the next round
                    }
                }
                RamTransferMode::DmaWrite => {
                    self.stat
                        .set(SpuStat::DATA_TRANSFER_BUSY_FLAG, self.in_dma_transfer);

                    self.stat.insert(
                        SpuStat::DATA_TRANSFER_USING_DMA | SpuStat::DATA_TRANSFER_DMA_WRITE_REQ,
                    );
                }
                RamTransferMode::DmaRead => {
                    self.stat
                        .set(SpuStat::DATA_TRANSFER_BUSY_FLAG, self.in_dma_transfer);
                    self.stat.insert(
                        SpuStat::DATA_TRANSFER_USING_DMA | SpuStat::DATA_TRANSFER_DMA_READ_REQ,
                    );
                }
            }

            let mut mixed_audio_left = 0;
            let mut mixed_audio_right = 0;

            let cd_left = self.cdrom_audio_buffer_left.pop_front().unwrap_or(0);
            let cd_right = self.cdrom_audio_buffer_right.pop_front().unwrap_or(0);
            self.spu_ram.push_cd_capture_samples(cd_left, cd_right);

            mixed_audio_left +=
                ((cd_left as i32 * self.cd_vol_left as i32) / 0x8000).clamp(-0x8000, 0x7FFF);
            mixed_audio_right +=
                ((cd_right as i32 * self.cd_vol_right as i32) / 0x8000).clamp(-0x8000, 0x7FFF);

            // TODO: implement correct order of handling voices (refer to above)
            for i in 0..24 {
                let pitch_mod = self.pitch_mod_channel_flag.get(i);
                let noise_mode = self.noise_channel_mode_flag.get(i);
                let _reverb_mode = self.reverb_channel_mode_flag.get(i);

                assert!(!pitch_mod);
                assert!(!noise_mode);
                //assert!(!_reverb_mode);

                // handle voices
                let (reached_endx, mono_output, left_output, right_output) =
                    self.voices[i].clock_voice(&self.spu_ram);

                // push the voice output to the capture buffer
                match i {
                    1 => self.spu_ram.push_voice_1_sample(mono_output),
                    3 => self.spu_ram.push_voice_3_sample(mono_output),
                    _ => {}
                }

                let final_left_output = (left_output * self.current_main_vol_left as i32 / 0x8000)
                    .clamp(-0x8000, 0x7FFF);
                mixed_audio_left += final_left_output;
                let final_right_output = (right_output * self.current_main_vol_right as i32
                    / 0x8000)
                    .clamp(-0x8000, 0x7FFF);
                mixed_audio_right += final_right_output;

                if reached_endx {
                    self.endx_flag.set(i, true);
                }
            }

            let (left, right) = if self.control.intersects(SpuControl::UNMUTE_SPU) {
                (
                    mixed_audio_left.clamp(-0x8000, 0x7FFF) as i16,
                    mixed_audio_right.clamp(-0x8000, 0x7FFF) as i16,
                )
            } else {
                (0, 0)
            };

            // convert i16 to f32
            let left = left as f32 / 0x8000 as f32;
            let right = right as f32 / 0x8000 as f32;

            self.out_audio_buffer.push(left);
            self.out_audio_buffer.push(right);

            if self
                .control
                .contains(SpuControl::SPU_ENABLE | SpuControl::IRQ9_ENABLE)
                && self.spu_ram.irq_flag.get()
            {
                self.stat.insert(SpuStat::IRQ_FLAG);
                interrupt_requester.request_spu();
            }
        }
    }

    pub(crate) fn add_cdrom_audio(&mut self, left: &[i16], right: &[i16]) {
        assert_eq!(left.len(), right.len());

        self.cdrom_audio_buffer_left.extend(left);
        self.cdrom_audio_buffer_right.extend(right);
    }

    pub fn take_audio_buffer(&mut self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.out_audio_buffer.len());
        out.extend_from_slice(&self.out_audio_buffer);
        self.out_audio_buffer.clear();
        out
    }

    pub fn print_state(&self) {
        println!("SPU State:");
        println!(
            "  Main Volume: Left: {:04X}, Right: {:04X}",
            self.main_vol_left, self.main_vol_right
        );
        println!(
            "  Reverb Volume: Left: {:04X}, Right: {:04X}",
            self.reverb_out_vol_left, self.reverb_out_vol_right
        );
        println!(
            "  CD Volume: Left: {:04X}, Right: {:04X}",
            self.cd_vol_left, self.cd_vol_right
        );
        println!(
            "  External Volume: Left: {:04X}, Right: {:04X}",
            self.external_vol_left, self.external_vol_right
        );
        println!(
            "  RAM Transfer Control: {:04X}, Address: {:04X}",
            self.ram_transfer_control, self.ram_transfer_address
        );
        println!("  Control: {:X?}, Stat: {:X?}", self.control, self.stat);
        println!("  Reverb Work Base: {:04X}", self.reverb_work_base);
        println!("  Reverb Config: {:02X?}", self.reverb_config);
        println!(
            "  IRQ Address: {:X}, IRQ Flag: {}",
            self.spu_ram.irq_address / 4,
            self.spu_ram.irq_flag.get()
        );
        println!();
        println!("  | {:^2} | {:^6} | {:^7} | {:^9} | {:^10} | {:^11} | {:^5} | {:^8} | {:^9} | {:^11} | {:^10} | {:^11} | {:^12} | {:^11} | {:^8} | {:^10} | {:^12} | {:^13} |", 
             "V#", "Key On", "Key Off", "Pitch Mod", "Noise Mode", "Reverb Mode", "Endx", 
             "Vol Left", "Vol Right", "Sample Rate", "Start Addr", "Repeat Addr", "Current Addr", "ADSR Config", 
             "ADSR Vol", "ADSR State", "Sample Index", "Pitch Counter");

        for i in 0..24 {
            println!("  | {:^2} | {:^6?} | {:^7?} | {:^9?} | {:^10?} | {:^11?} | {:^5?} | {:^8X} | {:^9X} | {:^11X} | {:^10X} | {:^11X} | {:^12X} | {:^11X} | {:^8X} | {:^10} | {:^12} | {:^13X} |", 
                i,
                self.voices[i].is_on,
                self.voices[i].is_off,
                self.pitch_mod_channel_flag.get(i),
                self.noise_channel_mode_flag.get(i),
                self.reverb_channel_mode_flag.get(i),
                self.endx_flag.get(i),
                self.voices[i].volume_left,
                self.voices[i].volume_right,
                self.voices[i].adpcm_sample_rate,
                self.voices[i].adpcm_start_address,
                self.voices[i].adpcm_repeat_address,
                self.voices[i].i_adpcm_current_address / 4,
                self.voices[i].adsr_config,
                self.voices[i].adsr_current_vol,
                format!("{:?}", self.voices[i].i_adsr_state), // fixes the alignment, idk why
                                                              // aligning with debug wasn't working
                self.voices[i].i_cached_sample_index,
                self.voices[i].i_adpcm_pitch_counter);
        }
    }
}

// DMA transfer
impl Spu {
    pub fn is_ready_for_dma(&mut self, write: bool) -> bool {
        self.in_dma_transfer = true;
        self.stat.insert(SpuStat::DATA_TRANSFER_BUSY_FLAG);

        if self.stat.intersects(SpuStat::DATA_TRANSFER_USING_DMA) {
            if write {
                self.stat.intersects(SpuStat::DATA_TRANSFER_DMA_WRITE_REQ)
            } else {
                self.stat.intersects(SpuStat::DATA_TRANSFER_DMA_READ_REQ)
            }
        } else {
            false
        }
    }

    pub fn dma_write_buf(&mut self, buf: &[u32]) {
        self.in_dma_transfer = true;
        self.stat.insert(SpuStat::DATA_TRANSFER_BUSY_FLAG);

        // finish this first
        if !self.write_data_fifo.is_empty() {
            for d in self.write_data_fifo.drain(..) {
                self.spu_ram[self.i_ram_transfer_address] = d;
                self.i_ram_transfer_address += 1;
                self.i_ram_transfer_address &= 0x3FFFF;
            }
        }

        for d in buf {
            let low = *d as u16;
            let high = (*d >> 16) as u16;
            self.spu_ram[self.i_ram_transfer_address] = low;
            self.i_ram_transfer_address += 1;
            self.i_ram_transfer_address &= 0x3FFFF;

            self.spu_ram[self.i_ram_transfer_address] = high;
            self.i_ram_transfer_address += 1;
            self.i_ram_transfer_address &= 0x3FFFF;
        }

        self.stat
            .remove(SpuStat::DATA_TRANSFER_DMA_WRITE_REQ | SpuStat::DATA_TRANSFER_USING_DMA);
    }

    pub fn dma_read_buf(&mut self, size: usize) -> Vec<u32> {
        self.in_dma_transfer = true;
        self.stat.insert(SpuStat::DATA_TRANSFER_BUSY_FLAG);

        let mut buf = Vec::with_capacity(size);

        for _ in 0..size {
            let low = self.spu_ram[self.i_ram_transfer_address];
            self.i_ram_transfer_address += 1;
            self.i_ram_transfer_address &= 0x3FFFF;

            let high = self.spu_ram[self.i_ram_transfer_address];
            self.i_ram_transfer_address += 1;
            self.i_ram_transfer_address &= 0x3FFFF;

            buf.push((high as u32) << 16 | low as u32);
        }

        self.stat
            .remove(SpuStat::DATA_TRANSFER_DMA_WRITE_REQ | SpuStat::DATA_TRANSFER_USING_DMA);
        buf
    }

    pub fn finish_dma(&mut self) {
        self.in_dma_transfer = false;
    }
}

impl BusLine for Spu {
    fn read_u32(&mut self, addr: u32) -> Result<u32> {
        match addr {
            0x000..=0x17F => Err(format!("SPU u32 read voice register {:03X}", addr)),
            0x180..=0x187 => Err(format!("SPU u32 read spu control {:03X}", addr)),
            0x188..=0x19F => Err(format!("SPU u32 read voice flags {:03X}", addr)),
            0x1A0..=0x1BF => Err(format!("SPU u32 read spu  control {:03X}", addr)),
            0x1C0..=0x1FF => Err(format!("SPU u32 read reverb configuration {:03X}", addr)),
            0x200..=0x25F => Err(format!("SPU u32 read voice internal reg {:03X}", addr)),
            0x260..=0x2FF => unreachable!("u32 read unknown {:03X}", addr),
            _ => unreachable!(),
        }
    }

    fn write_u32(&mut self, addr: u32, _data: u32) -> Result<()> {
        match addr {
            0x000..=0x17F => Err(format!("SPU u32 write voice register {:03X}", addr)),
            0x180..=0x187 => Err(format!("SPU u32 write spu control {:03X}", addr)),
            0x188..=0x19F => Err(format!("SPU u32 write voice flags {:03X}", addr)),
            0x1A0..=0x1BF => Err(format!("SPU u32 write spu  control {:03X}", addr)),
            0x1C0..=0x1FF => Err(format!("SPU u32 write reverb configuration {:03X}", addr)),
            0x200..=0x25F => Err(format!("SPU u32 write voice internal reg {:03X}", addr)),
            0x260..=0x2FF => unreachable!("u32 write unknown {:03X}", addr),
            _ => unreachable!(),
        }
    }

    fn read_u16(&mut self, addr: u32) -> Result<u16> {
        let r = match addr {
            0x000..=0x17E => {
                let reg = addr & 0xF;
                let voice_idx = (addr >> 4) as usize;
                match reg {
                    0x0 => self.voices[voice_idx].volume_left,
                    0x2 => self.voices[voice_idx].volume_right,
                    0x4 => self.voices[voice_idx].adpcm_sample_rate,
                    0x6 => self.voices[voice_idx].adpcm_start_address,
                    0x8 => self.voices[voice_idx].adsr_config.bits() as u16,
                    0xA => (self.voices[voice_idx].adsr_config.bits() >> 16) as u16,
                    0xC => self.voices[voice_idx].adsr_current_vol,
                    0xE => self.voices[voice_idx].adpcm_repeat_address,
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
            0x190 => self.pitch_mod_channel_flag.get_all() as u16,
            0x192 => (self.pitch_mod_channel_flag.get_all() >> 16) as u16,
            0x194 => self.noise_channel_mode_flag.get_all() as u16,
            0x196 => (self.noise_channel_mode_flag.get_all() >> 16) as u16,
            0x198 => self.reverb_channel_mode_flag.get_all() as u16,
            0x19A => (self.reverb_channel_mode_flag.get_all() >> 16) as u16,
            0x19C => self.endx_flag.get_all() as u16,
            0x19E => (self.endx_flag.get_all() >> 16) as u16,
            0x1A2 => self.reverb_work_base,
            0x1A4 => (self.spu_ram.irq_address / 4) as u16,
            0x1A6 => self.ram_transfer_address,
            0x1AA => self.control.bits(),
            0x1AC => self.ram_transfer_control,
            0x1AE => self.stat.bits(),
            0x1B0 => self.cd_vol_left,
            0x1B2 => self.cd_vol_right,
            0x1B4 => self.external_vol_left,
            0x1B6 => self.external_vol_right,
            0x1B8 => self.current_main_vol_left as u16,
            0x1BA => self.current_main_vol_right as u16,
            0x1C0..=0x1FE => self.reverb_config[(addr - 0x1C0) as usize / 2],
            0x200..=0x25E => {
                let voice_idx = ((addr >> 2) & 23) as usize;
                if addr & 0x2 == 0 {
                    self.voices[voice_idx].current_vol_left as u16
                } else {
                    self.voices[voice_idx].current_vol_right as u16
                }
            }
            0x1A0 | 0x1BC..=0x1BF | 0x260..=0x2FF => {
                log::warn!("Reading from unknown register {:03X}, returning 0...", addr);
                0
            }
            _ => unreachable!(),
        };
        Ok(r)
    }

    fn write_u16(&mut self, addr: u32, data: u16) -> Result<()> {
        match addr {
            0x000..=0x17E => {
                let reg = addr & 0xF;
                let voice_idx = (addr >> 4) as usize;
                log::info!("voice {}, reg {:01X} = {:04X}", voice_idx, reg, data);
                match reg {
                    0x0 => {
                        self.voices[voice_idx].volume_left = data;
                        // volume mode
                        if data & 0x8000 == 0 {
                            self.voices[voice_idx].current_vol_left = (data * 2) as i16;
                        }
                    }
                    0x2 => {
                        self.voices[voice_idx].volume_right = data;
                        // volume mode
                        if data & 0x8000 == 0 {
                            self.voices[voice_idx].current_vol_right = (data * 2) as i16;
                        }
                    }
                    0x4 => self.voices[voice_idx].adpcm_sample_rate = data,
                    0x6 => self.voices[voice_idx].adpcm_start_address = data,
                    0x8 => {
                        let f = self.voices[voice_idx].adsr_config.bits();
                        self.voices[voice_idx].adsr_config =
                            ADSRConfig::from_bits_retain((f & 0xFFFF0000) | data as u32);
                    }
                    0xA => {
                        let f = self.voices[voice_idx].adsr_config.bits();
                        self.voices[voice_idx].adsr_config =
                            ADSRConfig::from_bits_retain((f & 0xFFFF) | ((data as u32) << 16));
                    }
                    0xC => self.voices[voice_idx].adsr_current_vol = data,
                    0xE => self.voices[voice_idx].adpcm_repeat_address = data,
                    _ => unreachable!(),
                }
            }
            0x180 => {
                log::info!("main vol left = {:04X}", data);
                self.main_vol_left = data;
                // volume mode
                if data & 0x8000 == 0 {
                    self.current_main_vol_left = (data * 2) as i16;
                }
            }
            0x182 => {
                log::info!("main vol right = {:04X}", data);
                self.main_vol_right = data;
                // volume mode
                if data & 0x8000 == 0 {
                    self.current_main_vol_right = (data * 2) as i16;
                }
            }
            0x184 => {
                log::info!("reverb vol left = {:04X}", data);
                self.reverb_out_vol_left = data;
            }
            0x186 => {
                log::info!("reverb vol right = {:04X}", data);
                self.reverb_out_vol_right = data;
            }
            0x188 => {
                let f = self.key_on_flag.get_all();
                self.key_on_flag.bus_set_all((f & 0xFFFF0000) | data as u32);
                log::info!("key on flag = {:08X}", self.key_on_flag.get_all());

                for i in 0..16 {
                    if self.key_on_flag.get(i) {
                        self.endx_flag.set(i, false);
                        self.voices[i].key_on();
                    }
                }
            }
            0x18A => {
                let f = self.key_on_flag.get_all();
                self.key_on_flag
                    .bus_set_all((f & 0x0000FFFF) | ((data as u32) << 16));
                log::info!("key on flag = {:08X}", self.key_on_flag.get_all());

                for i in 16..24 {
                    if self.key_on_flag.get(i) {
                        self.endx_flag.set(i, false);
                        self.voices[i].key_on();
                    }
                }
            }
            0x18C => {
                let f = self.key_off_flag.get_all();
                self.key_off_flag
                    .bus_set_all((f & 0xFFFF0000) | data as u32);
                log::info!("key off flag = {:08X}", self.key_off_flag.get_all());

                for i in 0..16 {
                    if self.key_off_flag.get(i) {
                        self.voices[i].key_off();
                    }
                }
            }
            0x18E => {
                let f = self.key_off_flag.get_all();
                self.key_off_flag
                    .bus_set_all((f & 0x0000FFFF) | ((data as u32) << 16));
                log::info!("key off flag = {:08X}", self.key_off_flag.get_all());

                for i in 16..24 {
                    if self.key_off_flag.get(i) {
                        self.voices[i].key_off();
                    }
                }
            }
            0x190 => {
                let f = self.pitch_mod_channel_flag.get_all();
                self.pitch_mod_channel_flag
                    .bus_set_all((f & 0xFFFF0000) | data as u32);

                log::info!(
                    "pitch mod flag = {:08X}",
                    self.pitch_mod_channel_flag.get_all()
                );
            }
            0x192 => {
                let f = self.pitch_mod_channel_flag.get_all();
                self.pitch_mod_channel_flag
                    .bus_set_all((f & 0x0000FFFF) | ((data as u32) << 16));
                log::info!(
                    "pitch mod flag = {:08X}",
                    self.pitch_mod_channel_flag.get_all()
                );
            }
            0x194 => {
                let f = self.noise_channel_mode_flag.get_all();
                self.noise_channel_mode_flag
                    .bus_set_all((f & 0xFFFF0000) | data as u32);
                log::info!(
                    "noise channel mode flag = {:08X}",
                    self.noise_channel_mode_flag.get_all()
                );
            }
            0x196 => {
                let f = self.noise_channel_mode_flag.get_all();
                self.noise_channel_mode_flag
                    .bus_set_all((f & 0x0000FFFF) | ((data as u32) << 16));
                log::info!(
                    "noise channel mode flag = {:08X}",
                    self.noise_channel_mode_flag.get_all()
                );
            }
            0x198 => {
                let f = self.reverb_channel_mode_flag.get_all();
                self.reverb_channel_mode_flag
                    .bus_set_all((f & 0xFFFF0000) | data as u32);
                log::info!(
                    "reverb channel mode flag = {:08X}",
                    self.reverb_channel_mode_flag.get_all()
                );
            }
            0x19A => {
                let f = self.reverb_channel_mode_flag.get_all();
                self.reverb_channel_mode_flag
                    .bus_set_all((f & 0x0000FFFF) | ((data as u32) << 16));
                log::info!(
                    "reverb channel mode flag = {:08X}",
                    self.reverb_channel_mode_flag.get_all()
                );
            }
            // channel enable should be read only
            // writing to it will save the data, but it doesn't have any affect
            // on the hardware functionality, and will be overwritten by the hardware
            0x19C => {
                let f = self.endx_flag.get_all();
                self.endx_flag.bus_set_all((f & 0xFFFF0000) | data as u32);
            }
            0x19E => {
                let f = self.endx_flag.get_all();
                self.endx_flag
                    .bus_set_all((f & 0x0000FFFF) | ((data as u32) << 16));
            }
            0x1A2 => {
                log::info!("reverb work area start = {:04X}", data);
                self.reverb_work_base = data;
            }
            0x1A4 => {
                log::info!("irq address = {:04X}", data);
                self.spu_ram.irq_address = data as usize * 4;
            }
            0x1A6 => {
                log::info!("sound ram data transfer address {:04X}", data);
                self.ram_transfer_address = data;
                self.i_ram_transfer_address = data as usize * 4;
            }
            0x1A8 => {
                log::info!("sound ram data transfer fifo {:04X}", data);
                // TODO: this check is removed for now since the DMA uses
                //       the same buffer for writes
                //if self.data_fifo.len() == 32 {
                //    panic!("sound ram data transfer fifo overflow");
                //}
                self.write_data_fifo.push_back(data);
            }
            0x1AA => {
                self.control = SpuControl::from_bits_retain(data);

                // ack interrupt/clear flag
                if !self.control.intersects(SpuControl::IRQ9_ENABLE) {
                    self.stat.remove(SpuStat::IRQ_FLAG);
                }

                log::info!("spu control {:04X}", data);
            }
            0x1AC => {
                self.ram_transfer_control = data;
                // TODO: support more control modes
                //let control_mode = (data >> 1) & 7;
                //assert!(control_mode == 2);
            }
            0x1AE => log::warn!("u16 write SpuStat is not supported, ignoring..."),
            0x1B0 => {
                log::info!("cd volume left {:04X}", data);
                self.cd_vol_left = data;
            }
            0x1B2 => {
                log::info!("cd volume right {:04X}", data);
                self.cd_vol_right = data;
            }
            0x1B4 => self.external_vol_left = data,
            0x1B6 => self.external_vol_right = data,
            0x1B8 => self.current_main_vol_left = data as i16,
            0x1BA => self.current_main_vol_right = data as i16,
            0x1C0..=0x1FE => self.reverb_config[(addr - 0x1C0) as usize / 2] = data,
            // TODO: not sure if this is writable, since its internal current vol
            0x200..=0x25F => todo!("u16 write voice internal reg {:03X}", addr),
            0x1A0 | 0x1BC..=0x1BF | 0x260..=0x2FF => {
                log::warn!(
                    "Writing value {:04X} to unknown register {:03X}, ignoring...",
                    data,
                    addr
                )
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    fn read_u8(&mut self, _addr: u32) -> Result<u8> {
        todo!()
    }

    fn write_u8(&mut self, addr: u32, _data: u8) -> Result<()> {
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
