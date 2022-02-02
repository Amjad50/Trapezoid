use crate::memory::{interrupts::InterruptRequester, BusLine};
use bitflags::bitflags;

use std::{
    collections::VecDeque,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

// These delay is still very fast even for double speed, but 255 delay
// between read sectors is very fast, and the software cannot handle it.
const CDROM_COMMAND_DEFAULT_DELAY: u32 = 10000;
const CDROM_READ_COMMAND_DELAY: u32 = 20000;

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
    #[derive(Default)]
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
    #[derive(Default)]
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

/// Utility function to convert value from bcd format to normal
fn from_bcd(arg: u8) -> u8 {
    ((arg & 0xF0) >> 4) * 10 + (arg & 0x0F)
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
    pub fn clock(&mut self, interrupt_requester: &mut impl InterruptRequester, cycles: u32) {
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
            match cmd {
                0x01 => {
                    // GetStat
                    log::info!("cdrom cmd: GetStat");
                    // TODO: handle errors
                    assert!(self.status.bits & 0b101 == 0);

                    self.write_to_response_fifo(self.status.bits);
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
                    self.write_to_response_fifo(self.status.bits);
                    self.request_interrupt_0_7(3);

                    self.reset_command();
                }
                0x06 => {
                    // ReadN

                    if self.command_state.is_none() {
                        // FIRST

                        log::info!("cdrom cmd: ReadN");
                        self.do_seek();

                        self.write_to_response_fifo(self.status.bits);
                        self.request_interrupt_0_7(3);
                        // any data for now, just to proceed to SECOND
                        self.command_state = Some(0);

                        self.command_delay_timer += CDROM_READ_COMMAND_DELAY;
                    } else {
                        // SECOND

                        // wait until the data fifo buffer is empty
                        log::info!(
                            "cdrom cmd: ReadN: pushing sector {} to data fifo buffer",
                            self.cursor_sector_position
                        );

                        let start;
                        let end;
                        if self.mode.intersects(CdromMode::USE_WHOLE_SECTOR) {
                            // 12 to skip the sync patterns
                            start = self.cursor_sector_position * 2352 + 12;
                            end = start + 0x924;
                        } else {
                            // 12 + 3 + 1 + 8 to skip the sync patterns and the header
                            start = self.cursor_sector_position * 2352 + 24;
                            end = start + 0x800;
                        }
                        self.read_data_buffer
                            .extend_from_slice(&self.disk_data[start..end]);

                        // move to next sector
                        self.cursor_sector_position += 1;

                        self.write_to_response_fifo(self.status.bits);
                        self.request_interrupt_0_7(1);
                        self.command_delay_timer += CDROM_READ_COMMAND_DELAY;
                    }
                }
                0x09 => {
                    // Pause

                    // TODO: not sure how to do pause else on pause
                    //       since the buffer is cleared after every sector read
                    if self.command_state.is_none() {
                        // FIRST
                        log::info!("cdrom cmd: Pause");

                        self.write_to_response_fifo(self.status.bits);
                        self.request_interrupt_0_7(3);
                        // any data for now, just to proceed to SECOND
                        self.command_state = Some(0);
                    } else {
                        // SECOND
                        self.write_to_response_fifo(self.status.bits);
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

                        self.write_to_response_fifo(self.status.bits);
                        self.request_interrupt_0_7(3);
                        // any data for now, just to proceed to SECOND
                        self.command_state = Some(0);
                    } else {
                        // SECOND

                        self.write_to_response_fifo(self.status.bits);
                        self.request_interrupt_0_7(2);
                        self.reset_command();
                    }
                }
                0x0C => {
                    // Demute

                    log::info!("cdrom cmd: Demute");
                    self.write_to_response_fifo(self.status.bits);
                    self.request_interrupt_0_7(3);

                    self.reset_command();
                }
                0x0E => {
                    // Setmode

                    self.mode = CdromMode::from_bits_truncate(self.read_next_parameter().unwrap());
                    log::info!("cdrom cmd: Setmode({:?})", self.mode);

                    self.write_to_response_fifo(self.status.bits);
                    self.request_interrupt_0_7(3);

                    self.reset_command();
                }
                0x15 => {
                    // SeekL

                    if self.command_state.is_none() {
                        // FIRST
                        log::info!("cdrom cmd: SeekL");

                        self.do_seek();

                        self.write_to_response_fifo(self.status.bits);
                        self.request_interrupt_0_7(3);
                        // any data for now, just to proceed to SECOND
                        self.command_state = Some(0);
                    } else {
                        // SECOND
                        self.write_to_response_fifo(self.status.bits);
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
                        self.write_to_response_fifo(self.status.bits);
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
                        self.write_to_response_fifo(self.status.bits);
                        self.request_interrupt_0_7(3);
                        // any data for now, just to proceed to SECOND
                        self.command_state = Some(0);
                    } else {
                        // SECOND
                        self.write_to_response_fifo(self.status.bits);
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
                "cdrom seek: ({:02X}:{:02X}:{:02X}) => {:08X}",
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
        self.index | self.fifo_status.bits
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
                    // write 1.1 Sound Map Data Out
                }
                2 => {
                    // write 1.2 Sound Map Coding Info
                }
                3 => {
                    // write 1.3 Right-CD to Right-SPU Volume
                }
                _ => unreachable!(),
            },
            2 => match self.index {
                0 => self.write_to_parameter_fifo(data),
                1 => self.write_interrupt_enable_register(data),
                2 => {
                    // write 2.2 Left-CD to Left-SPU Volume
                }
                3 => {
                    // write 2.3 Right-CD to Left-SPU Volume
                }
                _ => unreachable!(),
            },
            3 => match self.index {
                0 => self.write_request_register(data),
                1 => self.write_interrupt_flag_register(data),
                2 => {
                    // write 3.2 Left-CD to Right-SPU Volume
                }
                3 => {
                    // write 3.3 Audio Volume Apply Changes
                }
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }
}
