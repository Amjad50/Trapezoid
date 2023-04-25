use crate::{
    memory::{interrupts::InterruptRequester, BusLine},
    spu::Spu,
};
use bitflags::bitflags;

use std::{
    collections::VecDeque,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const CDROM_COMMAND_DEFAULT_DELAY: u32 = 0x1100;
// This is to achive 75 sectors per second
// Which is calculated as 33868800 (CPU CYCLES) / 75
// because the default delay is always used, we subtract it from the delay needed
// to get the final delay
const CDROM_READ_COMMAND_DELAY: u32 = 0x6e400 - CDROM_COMMAND_DEFAULT_DELAY;

bitflags! {
    #[derive(Default)]
    struct FifosStatus: u8 {
        const ADPBUSY                 = 0b00000100;
        /// 1 when empty (triggered before writing 1st byte)
        const PARAMETER_FIFO_EMPTY    = 0b00001000;
        /// 0 when full (triggered after writing 16 bytes)
        const PARAMETER_FIFO_NOT_FULL = 0b00010000;
        /// 0 when empty (triggered after reading LAST byte)
        const RESPONSE_FIFO_NOT_EMPTY = 0b00100000;
        /// 0 when empty (triggered after reading LAST byte)
        const DATA_FIFO_NOT_EMPTY     = 0b01000000;
        /// busy transferring and executing the command
        const BUSY                    = 0b10000000;
    }
}

bitflags! {
    #[derive(Default, Debug)]
    struct CdromStatus: u8 {
        const ERROR        = 0b00000001;
        const MOTOR_ON     = 0b00000010;
        const SEEK_ERROR   = 0b00000100;
        const GETID_ERROR  = 0b00001000;
        const SHELL_OPEN   = 0b00010000;
        const READING_DATA = 0b00100000;
        const SEEKING      = 0b01000000;
        const PLAYING      = 0b10000000;
    }
}

bitflags! {
    #[derive(Default, Debug)]
    struct CdromMode: u8 {
        const DOUBLE_SPEED            = 0b10000000;
        const XA_ADPCM                = 0b01000000;
        const USE_WHOLE_SECTOR        = 0b00100000;
        const IGNORE_BIT              = 0b00010000;
        const XA_FILTER               = 0b00001000;
        const REPORT_INTERRUPT_ENABLE = 0b00000100;
        const AUTO_PAUSE              = 0b00000010;
        const CDDA                    = 0b00000001;
    }
}

bitflags! {
    #[derive(Default, Debug)]
    struct CodingInfo: u8 {
        const EMPHASIS                = 0b01000000;
        // (0=4 bits, 1=8 bits)
        const BITS_PER_SAMPLE         = 0b00010000;
        // (0=37800Hz, 1=18900Hz)
        const SAMPLE_RATE             = 0b00000100;
        const STEREO                  = 0b00000001;
        // const RESERVED             = 0b10101010;
    }
}

const ADPCM_TABLE_POS: &[i32; 4] = &[0, 60, 115, 98];
const ADPCM_TABLE_NEG: &[i32; 4] = &[0, 0, -52, -55];

/// This is very similar to what we are doing in the SPU, but the data
/// format is a bit different. That's why its split.
#[derive(Default, Clone, Copy)]
struct AdpcmDecoder {
    old: i32,
    older: i32,
}

impl AdpcmDecoder {
    /// Decode XA-ADPCM block
    ///
    /// `in_block` must be 128 bytes long
    pub fn decode_block(
        &mut self,
        in_block: &[u8],
        block_n: usize,
        sample_8bit: bool,
        out: &mut [i16; 28],
    ) {
        assert_eq!(in_block.len(), 128);
        assert!(
            (!sample_8bit) || block_n < 4,
            "invalid block_n {} for 8bit mode",
            block_n
        );

        let shift_filter = in_block[4 + block_n];

        // adpcm decoding...
        let mut shift_nibble = shift_filter & 0xf;
        if shift_nibble > 12 {
            shift_nibble = 9;
        }
        // The 4bit (or 8bit) samples are expanded to 16bit by left-shifting
        // them by 12 (or 8), that 16bit value is then right-shifted by the
        // selected 'shift' amount.
        let expand_shift = if sample_8bit { 8 } else { 12 };
        let shift_factor = expand_shift - shift_nibble;

        let filter = shift_filter >> 4;

        // only 4 filters supported in XA-ADPCM
        let filter = filter % 4;

        let f0 = ADPCM_TABLE_POS[filter as usize];
        let f1 = ADPCM_TABLE_NEG[filter as usize];

        for i in 0..28 {
            // if its 8bit sample, use the whole byte, else use half depending
            // on the block number
            let mut sample = if sample_8bit {
                let b = in_block[16 + i * 4 + block_n];
                b as i8 as i32
            } else {
                let b = in_block[16 + i * 4 + block_n / 2];
                let nibble_shift = (block_n & 1) * 4;
                let m = (b >> nibble_shift) & 0xF;
                if m & 0x8 != 0 {
                    ((m as u32) | 0xfffffff0) as i32
                } else {
                    m as i32
                }
            };

            // shift
            sample = sample << shift_factor;
            // apply adpcm filter
            sample = sample + (self.old * f0 + self.older * f1 + 32) / 64;
            sample = sample.clamp(-0x8000, 0x7fff);

            self.older = self.old;
            self.old = sample;

            out[i] = sample as i16;
        }
    }
}

const ZIGZAG_TABLE: [[i32; 29]; 7] = [
    [
        0, 0, 0, 0, 0, -0x0002, 0x000A, -0x0022, 0x0041, -0x0054, 0x0034, 0x0009, -0x010A, 0x0400,
        -0x0A78, 0x234C, 0x6794, -0x1780, 0x0BCD, -0x0623, 0x0350, -0x016D, 0x006B, 0x000A,
        -0x0010, 0x0011, -0x0008, 0x0003, -0x0001,
    ],
    [
        0, 0, 0, -0x0002, 0, 0x0003, -0x0013, 0x003C, -0x004B, 0x00A2, -0x00E3, 0x0132, -0x0043,
        -0x0267, 0x0C9D, 0x74BB, -0x11B4, 0x09B8, -0x05BF, 0x0372, -0x01A8, 0x00A6, -0x001B,
        0x0005, 0x0006, -0x0008, 0x0003, -0x0001, 0,
    ],
    [
        0, 0, -0x0001, 0x0003, -0x0002, -0x0005, 0x001F, -0x004A, 0x00B3, -0x0192, 0x02B1, -0x039E,
        0x04F8, -0x05A6, 0x7939, -0x05A6, 0x04F8, -0x039E, 0x02B1, -0x0192, 0x00B3, -0x004A,
        0x001F, -0x0005, -0x0002, 0x0003, -0x0001, 0, 0,
    ],
    [
        0, -0x0001, 0x0003, -0x0008, 0x0006, 0x0005, -0x001B, 0x00A6, -0x01A8, 0x0372, -0x05BF,
        0x09B8, -0x11B4, 0x74BB, 0x0C9D, -0x0267, -0x0043, 0x0132, -0x00E3, 0x00A2, -0x004B,
        0x003C, -0x0013, 0x0003, 0, -0x0002, 0, 0, 0,
    ],
    [
        -0x0001, 0x0003, -0x0008, 0x0011, -0x0010, 0x000A, 0x006B, -0x016D, 0x0350, -0x0623,
        0x0BCD, -0x1780, 0x6794, 0x234C, -0x0A78, 0x0400, -0x010A, 0x0009, 0x0034, -0x0054, 0x0041,
        -0x0022, 0x000A, -0x0001, 0, 0x0001, 0, 0, 0,
    ],
    [
        0x0002, -0x0008, 0x0010, -0x0023, 0x002B, 0x001A, -0x00EB, 0x027B, -0x0548, 0x0AFA,
        -0x16FA, 0x53E0, 0x3C07, -0x1249, 0x080E, -0x0347, 0x015B, -0x0044, -0x0017, 0x0046,
        -0x0023, 0x0011, -0x0005, 0, 0, 0, 0, 0, 0,
    ],
    [
        -0x0005, 0x0011, -0x0023, 0x0046, -0x0017, -0x0044, 0x015B, -0x0347, 0x080E, -0x1249,
        0x3C07, 0x53E0, -0x16FA, 0x0AFA, -0x0548, 0x027B, -0x00EB, 0x001A, 0x002B, -0x0023, 0x0010,
        -0x0008, 0x0002, 0, 0, 0, 0, 0, 0,
    ],
];

/// Performs interpolation and converts all audio
/// sample rates (18900Hz or 37800Hz) to 44100Hz
struct AdpcmInterpolator {
    samples_ringbuf: [i16; 0x20],
    samples_i: usize,
    sixstep_counter: usize,
}

impl Default for AdpcmInterpolator {
    fn default() -> Self {
        Self {
            samples_ringbuf: Default::default(),
            samples_i: Default::default(),
            sixstep_counter: 6, // start at 6 to be normal
        }
    }
}

impl AdpcmInterpolator {
    // TODO: optimize
    pub fn output_samples(&mut self, samples: &[i16], sample_rate_18900: bool, out: &mut Vec<i16>) {
        for &s in samples {
            // double the samples
            if sample_rate_18900 {
                self.samples_ringbuf[self.samples_i & 0x1F] = s;
                self.samples_ringbuf[(self.samples_i + 1) & 0x1F] = s;
                self.samples_i += 2;
                // since each sector must be 6 divisible, this shouldn't overflow
                self.sixstep_counter -= 2;
            } else {
                self.samples_ringbuf[self.samples_i & 0x1F] = s;
                self.samples_i += 1;
                self.sixstep_counter -= 1;
            }

            if self.sixstep_counter == 0 {
                self.sixstep_counter = 6;
                for i in 0..7 {
                    out.push(self.zigzag_interpolate(i))
                }
            }
        }
    }

    fn zigzag_interpolate(&mut self, table_i: usize) -> i16 {
        let mut sum = 0;

        for i in 1..30 {
            let sample = self.samples_ringbuf
                [((self.samples_i as isize - i as isize) & 0x1F) as usize]
                as i32;
            sum += sample * ZIGZAG_TABLE[table_i][i - 1] / 0x8000;
        }

        sum.clamp(-0x8000, 0x7fff) as i16
    }
}

/// Utility function to convert value from bcd format to normal
fn from_bcd(arg: u8) -> u8 {
    ((arg & 0xF0) >> 4) * 10 + (arg & 0x0F)
}

/// Utility function to convert value from from normal format to bcd
fn to_bcd(arg: u8) -> u8 {
    ((arg / 10) << 4) | (arg % 10)
}

pub struct Cdrom {
    index: u8,
    fifo_status: FifosStatus,
    status: CdromStatus,
    interrupt_enable: u8,
    interrupt_flag: u8,
    parameter_fifo: VecDeque<u8>,
    response_fifo: VecDeque<u8>,
    command: Option<u8>,
    /// A timer to delay execution of cdrom commands, in clock unit.
    /// This is needed because the bios is not designed to receive interrupt
    /// immediately after the command starts, so this is just a mitigation.
    command_delay_timer: u32,
    /// A way to be able to execute a command through more than one cycle,
    /// The type and design might change later
    command_state: Option<u8>,

    cue_file: Option<PathBuf>,
    cue_file_content: String,
    disk_data: Vec<u8>,

    // commands save buffer
    // params: minutes, seconds, sector (on entire disk)
    set_loc_params: Option<[u8; 3]>,
    // the current position on the disk
    cursor_sector_position: usize,

    mode: CdromMode,

    data_fifo_buffer: Vec<u8>,
    read_data_buffer: Vec<u8>,
    data_fifo_buffer_index: usize,

    filter_file: u8,
    filter_channel: u8,

    adpcm_decoder_left_mono: AdpcmDecoder,
    adpcm_decoder_right: AdpcmDecoder,
    adpcm_interpolator_left_mono: AdpcmInterpolator,
    adpcm_interpolator_right: AdpcmInterpolator,

    // the values cached by the input until applied
    input_cd_left_to_spu_left: u8,
    input_cd_left_to_spu_right: u8,
    input_cd_right_to_spu_left: u8,
    input_cd_right_to_spu_right: u8,

    // actual volumes
    vol_cd_left_to_spu_left: u8,
    vol_cd_left_to_spu_right: u8,
    vol_cd_right_to_spu_left: u8,
    vol_cd_right_to_spu_right: u8,

    adpcm_mute: bool,
}

impl Default for Cdrom {
    fn default() -> Self {
        Self {
            index: 0,
            fifo_status: FifosStatus::PARAMETER_FIFO_EMPTY | FifosStatus::PARAMETER_FIFO_NOT_FULL,
            status: CdromStatus::empty(),
            interrupt_enable: 0,
            interrupt_flag: 0,
            parameter_fifo: VecDeque::new(),
            response_fifo: VecDeque::new(),
            command: None,
            command_delay_timer: 0,
            command_state: None,
            cue_file: None,
            // empty vectors are not allocated
            cue_file_content: String::new(),
            disk_data: Vec::new(),

            set_loc_params: None,
            cursor_sector_position: 0,

            mode: CdromMode::empty(),

            data_fifo_buffer: Vec::new(),
            read_data_buffer: Vec::new(),
            data_fifo_buffer_index: 0,

            filter_file: 0,
            filter_channel: 0,

            adpcm_decoder_left_mono: AdpcmDecoder::default(),
            adpcm_decoder_right: AdpcmDecoder::default(),
            adpcm_interpolator_left_mono: AdpcmInterpolator::default(),
            adpcm_interpolator_right: AdpcmInterpolator::default(),

            input_cd_left_to_spu_left: 0,
            input_cd_left_to_spu_right: 0,
            input_cd_right_to_spu_left: 0,
            input_cd_right_to_spu_right: 0,

            vol_cd_left_to_spu_left: 0,
            vol_cd_left_to_spu_right: 0,
            vol_cd_right_to_spu_left: 0,
            vol_cd_right_to_spu_right: 0,

            adpcm_mute: false,
        }
    }
}

// file reading and handling
impl Cdrom {
    pub fn set_cue_file<P: AsRef<Path>>(&mut self, cue_file: P) {
        let a = cue_file.as_ref().to_path_buf();
        self.load_cue_file(&a);
        self.cue_file = Some(a);
    }

    fn load_cue_file(&mut self, cue_file: &Path) {
        // TODO: support parsing and loading the data based on the cue file
        // TODO: since some Cds can be large, try to do mmap
        // start motor
        self.status.insert(CdromStatus::MOTOR_ON);

        // read cue file
        let mut file = fs::File::open(cue_file).unwrap();
        let mut cue_content = String::new();
        file.read_to_string(&mut cue_content).unwrap();
        // parse cue format
        let mut parts = cue_content.split_whitespace();
        assert!(parts.next().unwrap() == "FILE");
        let mut bin_file_name = parts.next().unwrap().to_string();
        // must be in quotes
        assert!(bin_file_name.starts_with('"'));
        while !bin_file_name.ends_with('"') {
            bin_file_name.push_str(" ");
            bin_file_name.push_str(parts.next().unwrap());
        }
        // remove quotes
        bin_file_name = bin_file_name.trim_matches('"').to_string();
        assert!(parts.next().unwrap() == "BINARY");
        assert!(parts.next().unwrap() == "TRACK");
        assert!(parts.next().unwrap() == "01");
        assert!(parts.next().unwrap() == "MODE2/2352");
        assert!(parts.next().unwrap() == "INDEX");
        assert!(parts.next().unwrap() == "01");
        assert!(parts.next().unwrap() == "00:00:00");

        // load bin file
        let bin_file_path = cue_file.parent().unwrap().join(bin_file_name);
        log::info!("Loading bin file: {:?}", bin_file_path);
        let mut file = fs::File::open(bin_file_path).unwrap();
        // read to new vector
        let mut bin_file_content = Vec::new();
        file.read_to_end(&mut bin_file_content).unwrap();
        self.cue_file_content = cue_content;
        self.disk_data = bin_file_content;
    }
}

// clocking and commands
impl Cdrom {
    pub fn clock(
        &mut self,
        interrupt_requester: &mut impl InterruptRequester,
        spu: &mut Spu,
        cycles: u32,
    ) {
        if self.interrupt_flag & 7 != 0 {
            // pending interrupts, waiting for acknowledgement
            return;
        }
        if let Some(cmd) = self.command {
            // delay (this applies for all parts of the command)
            // If no delay is needed, it can be reset from the command itself
            if self.command_delay_timer > cycles + 1 {
                self.command_delay_timer -= cycles;
                return;
            }

            // reset the timer here, so that if a command needs to change the value
            // it can do so
            self.command_delay_timer = CDROM_COMMAND_DEFAULT_DELAY;
            // every command starts the motor (if its not already on)
            self.status.insert(CdromStatus::MOTOR_ON);
            match cmd {
                0x01 => {
                    // GetStat
                    log::info!("cdrom cmd: GetStat");
                    // TODO: handle errors
                    assert!(self.status.bits() & 0b101 == 0);

                    self.write_to_response_fifo(self.status.bits());
                    self.request_interrupt_0_7(3);

                    self.reset_command();
                }
                0x02 => {
                    // SetLoc

                    let mut params = [0; 3];
                    // minutes
                    params[0] = from_bcd(self.read_next_parameter().unwrap());
                    // seconds
                    params[1] = from_bcd(self.read_next_parameter().unwrap());
                    // sector
                    params[2] = from_bcd(self.read_next_parameter().unwrap());

                    self.set_loc_params = Some(params);

                    log::info!("cdrom cmd: SetLoc({:?})", params);
                    self.write_to_response_fifo(self.status.bits());
                    self.request_interrupt_0_7(3);

                    self.reset_command();
                }
                0x06 | 0x1B => {
                    // ReadN/ReadS

                    if self.command_state.is_none() {
                        // FIRST

                        log::info!("cdrom cmd: ReadN");
                        self.do_seek();

                        self.write_to_response_fifo(self.status.bits());
                        self.request_interrupt_0_7(3);
                        // This stores the value 0 if this is the first attemp
                        // to read a data sector, otherwise store any other value.
                        //
                        // Check later on the reading logic to know what this mean.
                        //
                        // 0 is the start value
                        self.command_state = Some(0);

                        self.command_delay_timer += if self.mode.intersects(CdromMode::DOUBLE_SPEED)
                        {
                            CDROM_READ_COMMAND_DELAY / 2
                        } else {
                            CDROM_READ_COMMAND_DELAY
                        };

                        // reset data buffer
                        self.read_data_buffer.clear();
                    } else {
                        let command_state = self.command_state.as_mut().unwrap();

                        // SECOND

                        let sector_start = self.cursor_sector_position * 2352;

                        // skip the sync bytes
                        let whole_sector = &self.disk_data[sector_start + 12..sector_start + 0x930];

                        // TODO: add filtering and coding info handling
                        let mode = whole_sector[3];
                        let file = whole_sector[4];
                        let channel = whole_sector[5] & 0x1F;
                        let submode = whole_sector[6];
                        let coding_info = whole_sector[7];
                        let submode_audio = submode & 0x4 != 0;
                        let submode_realtime = submode & 0x40 != 0;

                        // was the current sector read, and should we move to the next?
                        let mut sector_read = false;

                        let filter_match =
                            self.filter_file == file && self.filter_channel == channel;

                        // delivery options:
                        //   try_deliver_as_adpcm_sector:
                        //    TODO: reject if CD-DA AUDIO format
                        //    reject if sector isn't MODE2 format
                        //    reject if adpcm_disabled(setmode.6)
                        //    reject if filter_enabled(setmode.3) AND selected file/channel doesn't match
                        //    reject if submode isn't audio+realtime (bit2 and bit6 must be both set)
                        //    deliver: send sector to xa-adpcm decoder when passing above cases

                        // mode 2 is CD-XA mode, which is the mode of the sector
                        if mode == 2
                            && self.mode.intersects(CdromMode::XA_ADPCM)
                            && (!self.mode.intersects(CdromMode::XA_FILTER) || filter_match)
                            && submode_audio
                            && submode_realtime
                        {
                            *command_state = 0; // reset data delivery attempts

                            self.deliver_adpcm_to_spu(
                                self.cursor_sector_position,
                                CodingInfo::from_bits_retain(coding_info),
                                spu,
                            );
                            sector_read = true;

                        // TODO: for some reason, this doesn't work on CTR,
                        //       it expects to get data interrupts on other channels
                        //       when reading from XA interleaved sectors,
                        //       the current implementation doesn't do the below check to the
                        //       letter, it only does it in the second attempt, so the first
                        //       attempt will always send something.
                        //       Try to find what is the best approach to this
                        //
                        //  try_deliver_as_data_sector:
                        //    reject data-delivery if "try_deliver_as_adpcm_sector" did do adpcm-delivery
                        //    reject if filter_enabled(setmode.3) AND submode is audio+realtime (bit2+bit6)
                        //    1st delivery attempt: send INT1+data, unless there's another INT pending
                        //    delay, and retry at later time... but this time with file/channel checking!
                        //    reject if filter_enabled(setmode.3) AND selected file/channel doesn't match
                        //    2nd delivery attempt: send INT1+data, unless there's another INT pending
                        } else if *command_state == 0
                            || !self.mode.intersects(CdromMode::XA_FILTER)
                            || !(submode_audio && submode_realtime)
                                && (!self.mode.intersects(CdromMode::XA_FILTER) || filter_match)
                        {
                            // only refill the data if the buffer is taken, else
                            // just interrupt
                            //
                            // if we're on the second attempt, and the buffer is still not empty
                            // perform buffer overrun, i.e. replace the data of the current buffer
                            if self.read_data_buffer.is_empty() || *command_state == 1 {
                                // convert from cursor pos to sector,seconds,minutes
                                // for debugging
                                let sector = self.cursor_sector_position % 75;
                                let total_seconds = (self.cursor_sector_position / 75) + 2;
                                let minutes = total_seconds / 60;
                                let seconds = total_seconds % 60;

                                // wait until the data fifo buffer is empty
                                log::info!(
                                    "cdrom cmd: ReadN: pushing sector {} [{:02}:{:02}:{:02}] to data fifo buffer",
                                    self.cursor_sector_position,
                                    minutes,
                                    seconds,
                                    sector
                                );

                                let data = if self.mode.intersects(CdromMode::USE_WHOLE_SECTOR) {
                                    whole_sector
                                } else {
                                    // skip the sub header
                                    &whole_sector[12..12 + 0x800]
                                };

                                // if there is something, override it
                                self.read_data_buffer.clear();
                                self.read_data_buffer.extend_from_slice(&data);

                                *command_state = 0;
                                sector_read = true;
                            } else {
                                // switch between 1st and 2nd delivery attempt
                                // 0 and 1
                                *command_state += 1;
                            }

                            self.write_to_response_fifo(self.status.bits());
                            self.request_interrupt_0_7(1);
                        //  else:
                        //    ignore sector silently
                        } else {
                            *command_state = 0; // reset data delivery attempts

                            // skip sector
                            sector_read = true;
                        }

                        // if we haven't read, just wait the default delay and re-interrupt.
                        if sector_read {
                            self.cursor_sector_position += 1;
                        }
                        // perform full delay even if nothing was read
                        self.command_delay_timer += if self.mode.intersects(CdromMode::DOUBLE_SPEED)
                        {
                            CDROM_READ_COMMAND_DELAY / 2
                        } else {
                            CDROM_READ_COMMAND_DELAY
                        };
                    }
                }
                0x08 => {
                    // Stop

                    if self.command_state.is_none() {
                        // FIRST
                        log::info!("cdrom cmd: Stop");
                        self.status.remove(CdromStatus::MOTOR_ON);

                        self.write_to_response_fifo(self.status.bits());
                        self.request_interrupt_0_7(3);
                        // any data for now, just to proceed to SECOND
                        self.command_state = Some(0);
                    } else {
                        // SECOND
                        self.write_to_response_fifo(self.status.bits());
                        self.request_interrupt_0_7(2);
                        self.reset_command();
                    }
                }
                0x09 => {
                    // Pause

                    // TODO: not sure how to do pause else on pause
                    //       since the buffer is cleared after every sector read
                    if self.command_state.is_none() {
                        // FIRST
                        log::info!("cdrom cmd: Pause");

                        self.write_to_response_fifo(self.status.bits());
                        self.request_interrupt_0_7(3);
                        // any data for now, just to proceed to SECOND
                        self.command_state = Some(0);
                    } else {
                        // SECOND
                        self.write_to_response_fifo(self.status.bits());
                        self.request_interrupt_0_7(2);
                        self.reset_command();
                    }
                }
                0x0A => {
                    // Init

                    if self.command_state.is_none() {
                        // FIRST
                        log::info!("cdrom cmd: Init");

                        // TODO: check what exactly needs to be reset
                        //       do we reset all fifos?
                        //       do we reset setloc params and cursor position?

                        self.mode = CdromMode::empty();
                        // reset the status and run the motor
                        self.status = CdromStatus::MOTOR_ON;
                        // reset fifos
                        self.data_fifo_buffer.clear();
                        self.data_fifo_buffer_index = 0;
                        self.read_data_buffer.clear();
                        self.fifo_status.remove(FifosStatus::DATA_FIFO_NOT_EMPTY);
                        self.reset_parameter_fifo();
                        self.response_fifo.clear();
                        self.fifo_status
                            .remove(FifosStatus::RESPONSE_FIFO_NOT_EMPTY);

                        // reset cursor and set_loc positions
                        self.set_loc_params = None;
                        self.cursor_sector_position = 0;

                        self.write_to_response_fifo(self.status.bits());
                        self.request_interrupt_0_7(3);
                        // any data for now, just to proceed to SECOND
                        self.command_state = Some(0);
                    } else {
                        // SECOND

                        self.write_to_response_fifo(self.status.bits());
                        self.request_interrupt_0_7(2);
                        self.reset_command();
                    }
                }
                0x0C => {
                    // Demute

                    log::info!("cdrom cmd: Demute");
                    self.write_to_response_fifo(self.status.bits());
                    self.request_interrupt_0_7(3);

                    self.reset_command();
                }
                0x0D => {
                    // Setfilter
                    self.filter_file = self.read_next_parameter().unwrap();
                    self.filter_channel = self.read_next_parameter().unwrap();

                    log::info!(
                        "cdrom cmd: Setfilter: file: {}, channel: {}",
                        self.filter_file,
                        self.filter_channel
                    );

                    self.write_to_response_fifo(self.status.bits());
                    self.request_interrupt_0_7(3);

                    self.reset_command();
                }
                0x0E => {
                    // Setmode

                    self.mode = CdromMode::from_bits_retain(self.read_next_parameter().unwrap());
                    log::info!("cdrom cmd: Setmode({:?})", self.mode);

                    self.write_to_response_fifo(self.status.bits());
                    self.request_interrupt_0_7(3);

                    self.reset_command();
                }
                0x11 => {
                    // GetLocP

                    // TODO: fix when supporting multiple tracks
                    //       also, min, second, sectors below as well
                    let track = 1;
                    let index = 1;

                    let sector = self.cursor_sector_position % 75;
                    let total_seconds = (self.cursor_sector_position / 75) + 2;
                    let minutes = total_seconds / 60;
                    let seconds = total_seconds % 60;

                    self.write_slice_to_response_fifo(&[
                        track,
                        index,
                        // track
                        to_bcd(minutes as u8),
                        to_bcd(seconds as u8),
                        to_bcd(sector as u8),
                        // whole disk
                        to_bcd(minutes as u8),
                        to_bcd(seconds as u8),
                        to_bcd(sector as u8),
                    ]);

                    self.request_interrupt_0_7(3);

                    self.reset_command();
                }
                0x15 => {
                    // SeekL

                    if self.command_state.is_none() {
                        // FIRST
                        log::info!("cdrom cmd: SeekL");

                        self.do_seek();

                        self.write_to_response_fifo(self.status.bits());
                        self.request_interrupt_0_7(3);
                        // any data for now, just to proceed to SECOND
                        self.command_state = Some(0);
                    } else {
                        // SECOND
                        self.write_to_response_fifo(self.status.bits());
                        self.request_interrupt_0_7(2);
                        self.reset_command();
                    }
                }
                0x19 => {
                    // Test
                    let test_code = self.read_next_parameter().unwrap();
                    log::info!("cdrom cmd: Test({:02x})", test_code);
                    self.execute_test(test_code);

                    self.reset_command();
                }
                0x1A => {
                    // GetID

                    if self.command_state.is_none() {
                        // FIRST
                        log::info!("cdrom cmd: GetID");
                        self.write_to_response_fifo(self.status.bits());
                        self.request_interrupt_0_7(3);
                        // any data for now, just to proceed to SECOND
                        self.command_state = Some(0);
                    } else {
                        // SECOND
                        // TODO: rewrite GetID implementation to fill
                        //       all the details correctly from the state of the cdrom
                        let (response, interrupt) = if self.cue_file.is_some() {
                            // last byte is the region code identifier
                            // A(0x41): NTSC
                            // E(0x45): PAL
                            // I(0x49): JP
                            (&[0x02, 0x00, 0x20, 0x00, 0x53, 0x43, 0x45, 0x41], 2)
                        } else {
                            //  5 interrupt means error
                            (&[0x08, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], 5)
                        };

                        self.write_slice_to_response_fifo(response);
                        self.request_interrupt_0_7(interrupt);
                        self.reset_command();
                    }
                }
                0x1E => {
                    // GetToc

                    if self.command_state.is_none() {
                        // FIRST
                        log::info!("cdrom cmd: GetToc");
                        self.write_to_response_fifo(self.status.bits());
                        self.request_interrupt_0_7(3);
                        // any data for now, just to proceed to SECOND
                        self.command_state = Some(0);
                    } else {
                        // SECOND
                        self.write_to_response_fifo(self.status.bits());
                        self.request_interrupt_0_7(2);
                        self.reset_command();
                    }
                }
                _ => todo!("cmd={:02X},state={:?}", cmd, self.command_state),
            }

            // fire irq only if the interrupt is enabled
            if self.interrupt_flag & self.interrupt_enable != 0 {
                interrupt_requester.request_cdrom();
            }
        }
    }

    // because of `&self` and `&mut self` conflict, we can't pass the
    // sector data directly (even though we already have it).
    // TODO: look to see if there is a better way for this
    fn deliver_adpcm_to_spu(
        &mut self,
        sector_position: usize,
        coding_info: CodingInfo,
        spu: &mut Spu,
    ) {
        let sector_start = sector_position * 2352;
        let data = &self.disk_data[sector_start + 24..sector_start + 24 + 0x900];

        let sample_8bit = coding_info.intersects(CodingInfo::BITS_PER_SAMPLE);

        // TODO: try to use static allocation/slab or anything that is not heap intensive
        let mut cd_audio_left: Vec<i16> = Vec::new();
        let mut cd_audio_right: Vec<i16> = Vec::new();

        // contain 0x12 portions of size 128 bytes.
        for i in 0..0x12 {
            let offset = i * 128;
            let portion = &data[offset..offset + 128];

            let mut block = 0;
            let mut temp_block = [0; 28];
            loop {
                self.adpcm_decoder_left_mono.decode_block(
                    portion,
                    block,
                    sample_8bit,
                    &mut temp_block,
                );
                self.adpcm_interpolator_left_mono.output_samples(
                    &temp_block,
                    coding_info.intersects(CodingInfo::SAMPLE_RATE),
                    &mut cd_audio_left,
                );

                if coding_info.intersects(CodingInfo::STEREO) {
                    block += 1;
                    self.adpcm_decoder_right.decode_block(
                        portion,
                        block,
                        sample_8bit,
                        &mut temp_block,
                    );

                    self.adpcm_interpolator_right.output_samples(
                        &temp_block,
                        coding_info.intersects(CodingInfo::SAMPLE_RATE),
                        &mut cd_audio_right,
                    );
                }
                block += 1;

                if (block == 4 && sample_8bit) || block == 8 {
                    break;
                }
            }
        }
        let mut spu_audio_left = vec![0; cd_audio_left.len()];
        let mut spu_audio_right = vec![0; cd_audio_left.len()];

        let (audio_left, audio_right) = if coding_info.intersects(CodingInfo::STEREO) {
            (&cd_audio_left, &cd_audio_right)
        } else {
            (&cd_audio_left, &cd_audio_left)
        };

        if !self.adpcm_mute {
            for (i, (&left, &right)) in audio_left.iter().zip(audio_right.iter()).enumerate() {
                let l = (left as i32 * self.vol_cd_left_to_spu_left as i32 / 0x80)
                    + (right as i32 * self.vol_cd_right_to_spu_left as i32 / 0x80);
                let r = (left as i32 * self.vol_cd_left_to_spu_right as i32 / 0x80)
                    + (right as i32 * self.vol_cd_right_to_spu_right as i32 / 0x80);

                spu_audio_left[i] = l.clamp(-0x8000, 0x7FFF) as i16;
                spu_audio_right[i] = r.clamp(-0x8000, 0x7FFF) as i16;
            }
        }

        spu.add_cdrom_audio(&spu_audio_left, &spu_audio_right);
    }

    fn execute_test(&mut self, test_code: u8) {
        match test_code {
            0x20 => {
                self.write_slice_to_response_fifo(&[0x99u8, 0x02, 0x01, 0xC3]);
                self.request_interrupt_0_7(3);
            }
            _ => todo!(),
        }
    }

    #[inline]
    fn do_seek(&mut self) {
        if let Some(params) = self.set_loc_params {
            // setting the position from the setLoc data
            let minutes = params[0] as usize;
            let seconds = params[1] as usize;
            let sector = params[2] as usize;

            let total_seconds = minutes * 60 + seconds;
            // there is an missing 2 seconds offset (for some reason)
            assert!(total_seconds >= 2);
            self.cursor_sector_position = (total_seconds - 2) * 75 + sector;

            log::info!(
                "cdrom seek: ({:02}:{:02}:{:02}) => {:08X}",
                minutes,
                seconds,
                sector,
                self.cursor_sector_position
            );

            self.set_loc_params = None;
        }
    }

    fn put_command(&mut self, cmd: u8) {
        self.command = Some(cmd);
        self.command_delay_timer = CDROM_COMMAND_DEFAULT_DELAY;
        self.command_state = None;
        self.fifo_status.insert(FifosStatus::BUSY);
    }

    fn reset_command(&mut self) {
        self.command = None;
        self.command_delay_timer = 0;
        self.command_state = None;
        self.fifo_status.remove(FifosStatus::BUSY);
    }
}

impl Cdrom {
    fn read_index_status(&self) -> u8 {
        self.index | self.fifo_status.bits()
    }

    fn write_interrupt_enable_register(&mut self, data: u8) {
        self.interrupt_enable = data & 0x1F;
        log::info!(
            "2.1 write interrupt enable register value={:02X}",
            self.interrupt_enable
        );
    }

    fn read_interrupt_enable_register(&self) -> u8 {
        (self.interrupt_enable & 0x1F) | 0xE0
    }

    fn write_interrupt_flag_register(&mut self, data: u8) {
        log::info!("3.1 write interrupt flag register value={:02X}", data);
        let interrupts_flag_to_ack = data & 0x1F;
        self.interrupt_flag &= !interrupts_flag_to_ack;

        if data & 0x40 != 0 {
            self.reset_parameter_fifo();
        }
    }

    fn read_interrupt_flag_register(&self) -> u8 {
        (self.interrupt_flag & 0x1F) | 0xE0
    }

    fn write_command_register(&mut self, data: u8) {
        log::info!("1.0 writing to command register cmd={:02X}", data);
        self.put_command(data)
    }

    fn reset_parameter_fifo(&mut self) {
        self.fifo_status.insert(FifosStatus::PARAMETER_FIFO_EMPTY);
        self.fifo_status
            .insert(FifosStatus::PARAMETER_FIFO_NOT_FULL);
        self.parameter_fifo.clear();
    }

    fn write_to_parameter_fifo(&mut self, data: u8) {
        if self.parameter_fifo.is_empty() {
            self.fifo_status.remove(FifosStatus::PARAMETER_FIFO_EMPTY);
        } else if self.parameter_fifo.len() == 15 {
            self.fifo_status
                .remove(FifosStatus::PARAMETER_FIFO_NOT_FULL);
        }
        log::info!("2.0 writing to parameter fifo={:02X}", data);

        self.parameter_fifo.push_back(data);
    }

    fn read_next_parameter(&mut self) -> Option<u8> {
        let out = self.parameter_fifo.pop_front();
        if self.parameter_fifo.is_empty() {
            self.fifo_status.insert(FifosStatus::PARAMETER_FIFO_EMPTY);
        } else if self.parameter_fifo.len() == 15 {
            self.fifo_status
                .insert(FifosStatus::PARAMETER_FIFO_NOT_FULL);
        }

        out
    }

    fn write_to_response_fifo(&mut self, data: u8) {
        if self.response_fifo.is_empty() {
            self.fifo_status
                .insert(FifosStatus::RESPONSE_FIFO_NOT_EMPTY);
        }
        log::info!("writing to response fifo={:02X}", data);

        self.response_fifo.push_back(data);
    }

    fn write_slice_to_response_fifo(&mut self, data: &[u8]) {
        if self.response_fifo.is_empty() {
            self.fifo_status
                .insert(FifosStatus::RESPONSE_FIFO_NOT_EMPTY);
        }
        log::info!("writing to response fifo={:02X?}", data);

        self.response_fifo.extend(data);
    }

    fn read_next_response(&mut self) -> u8 {
        let out = self.response_fifo.pop_front();

        if self.response_fifo.is_empty() {
            self.fifo_status
                .remove(FifosStatus::RESPONSE_FIFO_NOT_EMPTY);
        }

        // TODO: handle reading while being empty
        out.unwrap()
    }

    fn request_interrupt_0_7(&mut self, int_value: u8) {
        self.interrupt_flag &= !0x7;
        self.interrupt_flag |= int_value & 0x7;
    }

    fn write_request_register(&mut self, data: u8) {
        log::info!("3.0 writing to request register value={:02X}", data);
        // TODO: implement command start interrupt on next command
        assert!(data & 0x20 == 0);
        if data & 0x80 != 0 {
            // want data
            // this buffer should be set by Read commands
            if !self.read_data_buffer.is_empty() {
                self.data_fifo_buffer
                    .extend_from_slice(&self.read_data_buffer);
                self.read_data_buffer.clear();
                self.fifo_status.insert(FifosStatus::DATA_FIFO_NOT_EMPTY);
            }
        } else {
            log::info!("clearing data fifo buffer");
            self.data_fifo_buffer_index = 0;
            self.data_fifo_buffer.clear();
            self.fifo_status.remove(FifosStatus::DATA_FIFO_NOT_EMPTY);
        }
    }

    // TODO: dma should read a buffer directly from here
    fn read_next_data_fifo(&mut self) -> u8 {
        assert!(!self.data_fifo_buffer.is_empty());

        let out = self.data_fifo_buffer[self.data_fifo_buffer_index];
        self.data_fifo_buffer_index += 1;
        if self.data_fifo_buffer_index == self.data_fifo_buffer.len() {
            log::info!("data fifo buffer finished");
            self.data_fifo_buffer.clear();
            self.data_fifo_buffer_index = 0;
            self.fifo_status.remove(FifosStatus::DATA_FIFO_NOT_EMPTY);
        }
        out
    }
}

impl BusLine for Cdrom {
    fn read_u32(&mut self, _addr: u32) -> u32 {
        todo!()
    }

    fn write_u32(&mut self, _addr: u32, _data: u32) {
        todo!()
    }

    fn read_u16(&mut self, addr: u32) -> u16 {
        assert!(addr == 2);

        todo!()
    }

    fn write_u16(&mut self, _addr: u32, _data: u16) {
        todo!()
    }

    fn read_u8(&mut self, addr: u32) -> u8 {
        match addr {
            0 => self.read_index_status(),
            1 => self.read_next_response(),
            2 => self.read_next_data_fifo(),
            3 => match self.index & 1 {
                0 => self.read_interrupt_enable_register(),
                1 => self.read_interrupt_flag_register(),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }

    fn write_u8(&mut self, addr: u32, data: u8) {
        match addr {
            0 => {
                self.index = data & 3;
            }
            1 => match self.index {
                0 => self.write_command_register(data),
                1 => {
                    todo!("write 1.1 Sound Map Data Out");
                }
                2 => {
                    todo!("write 1.2 Sound Map Coding Info");
                }
                3 => {
                    self.input_cd_right_to_spu_right = data;
                }
                _ => unreachable!(),
            },
            2 => match self.index {
                0 => self.write_to_parameter_fifo(data),
                1 => self.write_interrupt_enable_register(data),
                2 => {
                    self.input_cd_left_to_spu_left = data;
                }
                3 => {
                    self.input_cd_right_to_spu_left = data;
                }
                _ => unreachable!(),
            },
            3 => match self.index {
                0 => self.write_request_register(data),
                1 => self.write_interrupt_flag_register(data),
                2 => {
                    // write 3.2 Left-CD to Right-SPU Volume
                    self.input_cd_left_to_spu_right = data;
                }
                3 => {
                    // write 3.3 Audio Volume Apply Changes
                    self.adpcm_mute = data & 1 == 1;
                    // apply volumes
                    if data & 0x20 != 0 {
                        log::info!("cd volume applied, muted: {}", self.adpcm_mute);
                        log::info!("l -> l {:02X}", self.input_cd_left_to_spu_left);
                        log::info!("l -> r {:02X}", self.input_cd_left_to_spu_right);
                        log::info!("r -> l {:02X}", self.input_cd_right_to_spu_left);
                        log::info!("r -> r {:02X}", self.input_cd_right_to_spu_right);

                        self.vol_cd_left_to_spu_left = self.input_cd_left_to_spu_left;
                        self.vol_cd_left_to_spu_right = self.input_cd_left_to_spu_right;
                        self.vol_cd_right_to_spu_left = self.input_cd_right_to_spu_left;
                        self.vol_cd_right_to_spu_right = self.input_cd_right_to_spu_right;
                    }
                }
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }
}
