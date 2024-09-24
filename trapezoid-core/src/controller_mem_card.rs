use crate::memory::{interrupts::InterruptRequester, BusLine, Result};
use bitflags::bitflags;

use std::collections::VecDeque;

#[derive(Clone, Copy)]
pub enum DigitalControllerKey {
    Select,
    L3,
    R3,
    Start,
    Up,
    Right,
    Down,
    Left,
    L2,
    R2,
    L1,
    R1,
    Triangle,
    Circle,
    X,
    Square,
}

impl DigitalControllerKey {
    fn mask(&self) -> u16 {
        1 << *self as u16
    }
}

const JOY_CTRL_ACKKNOWLEDGE: u16 = 0b0000000000010000;
const JOY_CTRL_RESET: u16 = 0b0000000001000000;
bitflags! {
    #[derive(Default, Debug)]
    struct JoyControl: u16 {
        const TX_ENABLE            = 0b0000000000000001;
        const JOY_SELECT           = 0b0000000000000010;
        const RX_FORCE_ENABLE      = 0b0000000000000100;
        // ACKKNOWLEDGE and RESET are write only
        // const ACKKNOWLEDGE      = 0b0000000000010000;
        // const RESET             = 0b0000000001000000;
        const RX_INTERRUPT_MODE    = 0b0000001100000000;
        const TX_INTERRUPT_ENABLE  = 0b0000010000000000;
        const RX_INTERRUPT_ENABLE  = 0b0000100000000000;
        const ACK_INTERRUPT_ENABLE = 0b0001000000000000;
        const JOY_SLOT             = 0b0010000000000000;
        const UNKNOWN              = 0b0000000000101000;
        // const NOT_USED          = 0b1100000010000000;
    }
}

impl JoyControl {
    fn tx_enable(&self) -> bool {
        self.intersects(Self::TX_ENABLE)
    }

    fn rx_force_enable(&self) -> bool {
        self.intersects(Self::RX_FORCE_ENABLE)
    }

    fn joy_selected(&self) -> bool {
        self.intersects(Self::JOY_SELECT)
    }

    fn ack_interrupt_enable(&self) -> bool {
        self.intersects(Self::ACK_INTERRUPT_ENABLE)
    }

    fn joy_slot(&self) -> u8 {
        self.intersects(Self::JOY_SLOT) as u8
    }
}

bitflags! {
    #[derive(Default, Debug)]
    struct JoyMode: u16 {
        const BAUDRATE_RELOAD_FACTOR = 0b0000000000000011;
        const CHARACTER_LENGTH       = 0b0000000000001100;
        const PARITY_ENABLE          = 0b0000000000010000;
        const PARITY_TYPE            = 0b0000000000100000;
        const CLK_OUTPUT_POLARITY    = 0b0000000100000000;
        // const NOT_USED            = 0b1111111011000000;
    }
}

impl JoyMode {
    fn baudrate_reload_factor_shift(&self) -> u32 {
        let bits = (self.bits() & Self::BAUDRATE_RELOAD_FACTOR.bits()) as u32;

        if bits == 1 {
            0
        } else {
            // 0 => 0 (MUL1)
            // 2 => 4 (MUL16)
            // 3 => 6 (MUL64)
            bits * 2
        }
    }

    fn character_length(&self) -> u8 {
        let bits = ((self.bits() & Self::CHARACTER_LENGTH.bits()) >> 2) as u8;

        5 + bits
    }

    fn clk_idle_on_high(&self) -> bool {
        !self.intersects(Self::CLK_OUTPUT_POLARITY)
    }
}

bitflags! {
    #[derive(Default)]
    struct JoyStat: u32 {
        const TX_READY_1             = 0b0000000000000001;
        const RX_FIFO_NOT_EMPTY      = 0b0000000000000010;
        const TX_READY_2             = 0b0000000000000100;
        const RX_PARITY_ERROR        = 0b0000000000001000;
        const ACK_INPUT_LEVEL_LOW    = 0b0000000010000000;
        const INTERRUPT_REQUEST      = 0b0000001000000000;
        // const NOT_USED            = 0b0000010101110000;
        // the rest is for the timer
    }
}

mod controller {
    #[derive(Debug, Clone, Copy)]
    pub enum ControllerMode {
        ReadButtons,
        Config,
        SetLed,
        GetLed,
        SetRumble,
        GetWhateverValues,
        GetVariableResponseA,
        GetVariableResponseB,

        /// Unknown that will always return 6 zeros
        Unknown60,
        /// Unknown that will return 4 zeros, one, then zero
        Unknown4010,
    }

    /// Emulate Digital pad controller communication
    pub struct Controller {
        state: u8,
        device_id: u16,
        digital_switches: u16,
        connected: bool,
        current_mode: ControllerMode,
        in_config: bool,

        led: bool,
        rumble_config: [u8; 6],

        /// Internal value with many purposes in the input state flow
        /// Used to store a value that may be used later in the flow
        cache_value: u8,
    }

    impl Controller {
        pub fn new(connected: bool) -> Self {
            Self {
                state: 0,
                in_config: false,
                current_mode: ControllerMode::ReadButtons,
                device_id: 0x5A41,        // digital controller
                digital_switches: 0xFFFF, // all released
                connected,

                led: false,
                rumble_config: [0xFF; 6],
                cache_value: 0,
            }
        }

        pub fn change_key_state(&mut self, key: super::DigitalControllerKey, pressed: bool) {
            let mask = key.mask();

            if pressed {
                self.digital_switches &= !mask;
            } else {
                self.digital_switches |= mask;
            }
        }

        pub fn start_access(&mut self) -> u8 {
            if self.connected {
                self.state = 1;
                0
            } else {
                0xFF
            }
        }

        fn exchange_bytes_normal(&mut self, inp: u8) -> (u8, bool) {
            match self.state {
                1 => {
                    self.current_mode = match inp {
                        0x42 => ControllerMode::ReadButtons,
                        0x43 => ControllerMode::Config,
                        _ => todo!("Controller first input {:02X} is not supported", inp),
                    };

                    self.state = 2;
                    ((self.device_id & 0xFF) as u8, false)
                }
                2 => {
                    // if `inp == 1`, then `multitap` is enabled
                    // but this is not a multitap controller, so will return
                    // the normal `device id`
                    assert!(inp == 0 || inp == 1);
                    self.state = 3;
                    (((self.device_id >> 8) & 0xFF) as u8, false)
                }
                3 => {
                    match self.current_mode {
                        ControllerMode::ReadButtons => {
                            // TODO: handle rumble
                        }
                        ControllerMode::Config => {
                            assert!(inp == 1 || inp == 0);
                            self.cache_value = inp;
                        }
                        _ => unreachable!(),
                    }
                    self.state = 4;
                    ((self.digital_switches & 0xFF) as u8, false)
                }
                4 => {
                    match self.current_mode {
                        ControllerMode::ReadButtons => {
                            // TODO: handle rumble
                        }
                        ControllerMode::Config => {
                            assert_eq!(inp, 0);
                            self.in_config = self.cache_value == 1;
                        }
                        _ => unreachable!(),
                    }
                    self.state = 0;
                    (((self.digital_switches >> 8) & 0xFF) as u8, true)
                }
                // TODO: handle for analog input
                _ => unreachable!(),
            }
        }

        fn exchange_bytes_config(&mut self, inp: u8) -> (u8, bool) {
            match self.state {
                1 => {
                    self.current_mode = match inp {
                        0x40 | 0x41 | 0x49 | 0x4A | 0x4B | 0x4E | 0x4F => ControllerMode::Unknown60,
                        0x42 => ControllerMode::ReadButtons,
                        0x43 => ControllerMode::Config,
                        0x44 => ControllerMode::SetLed,
                        0x45 => ControllerMode::GetLed,
                        0x46 => ControllerMode::GetVariableResponseA,
                        0x47 => ControllerMode::GetWhateverValues,
                        0x48 => ControllerMode::Unknown4010,
                        0x4C => ControllerMode::GetVariableResponseB,
                        0x4D => ControllerMode::SetRumble,
                        _ => todo!("unknown controller mode: {:02X}", inp),
                    };

                    self.state = 2;
                    (0xF3, false)
                }
                2 => {
                    // if `inp == 1`, then `multitap` is enabled
                    // but this is not a multitap controller, so will return
                    // the normal `device id`
                    assert!(inp == 0 || inp == 1);
                    self.state = 3;
                    (0x5A, false)
                }
                3 => {
                    let ret = match self.current_mode {
                        ControllerMode::ReadButtons => {
                            // TODO: handle rumble
                            (self.digital_switches & 0xFF) as u8
                        }
                        ControllerMode::Config => {
                            assert!(inp == 1 || inp == 0);
                            self.cache_value = inp;
                            0
                        }
                        ControllerMode::SetLed => {
                            assert!(inp == 1 || inp == 0);
                            self.cache_value = inp;
                            0
                        }
                        ControllerMode::GetLed => {
                            assert!(inp == 0);
                            1
                        }
                        ControllerMode::GetVariableResponseA => {
                            self.cache_value = inp;
                            0
                        }
                        ControllerMode::GetVariableResponseB => {
                            // used to identify dual shock controllers
                            self.cache_value = match inp {
                                0 => 4,
                                1 => 7,
                                _ => 0,
                            };
                            0
                        }
                        ControllerMode::GetWhateverValues
                        | ControllerMode::Unknown60
                        | ControllerMode::Unknown4010 => {
                            assert!(inp == 0);
                            0
                        }
                        ControllerMode::SetRumble => {
                            let ret = self.rumble_config[0];
                            self.rumble_config[0] = inp;
                            ret
                        }
                    };
                    self.state = 4;
                    (ret, false)
                }
                4 => {
                    let ret = match self.current_mode {
                        ControllerMode::ReadButtons => {
                            // TODO: handle rumble
                            ((self.digital_switches >> 8) & 0xFF) as u8
                        }
                        ControllerMode::Config => {
                            assert_eq!(inp, 0);
                            0
                        }
                        ControllerMode::SetLed => {
                            // only apply LED if `inp == 2`
                            if inp == 2 {
                                self.led = self.cache_value == 1;
                            }
                            // Side effect reset rumble to 0xFF
                            self.rumble_config = [0xFF; 6];
                            0
                        }
                        ControllerMode::GetLed => {
                            assert!(inp == 0);
                            2
                        }
                        ControllerMode::GetVariableResponseA
                        | ControllerMode::GetVariableResponseB
                        | ControllerMode::GetWhateverValues
                        | ControllerMode::Unknown60
                        | ControllerMode::Unknown4010 => {
                            assert!(inp == 0);
                            0
                        }
                        ControllerMode::SetRumble => {
                            let ret = self.rumble_config[1];
                            self.rumble_config[1] = inp;
                            ret
                        }
                    };
                    self.state = 5;
                    (ret, false)
                }
                5 => {
                    let ret = match self.current_mode {
                        ControllerMode::GetLed => self.led as u8,
                        ControllerMode::GetWhateverValues => 2,
                        ControllerMode::GetVariableResponseA => match self.cache_value {
                            0 | 1 => 1,
                            _ => 0,
                        },
                        ControllerMode::SetRumble => {
                            let ret = self.rumble_config[2];
                            self.rumble_config[2] = inp;
                            ret
                        }
                        _ => 0,
                    };
                    self.state = 6;
                    (ret, false)
                }
                6 => {
                    let ret = match self.current_mode {
                        ControllerMode::GetLed => 2,
                        ControllerMode::GetVariableResponseA => match self.cache_value {
                            0 => 2,
                            1 => 1,
                            _ => 0,
                        },
                        ControllerMode::GetVariableResponseB => self.cache_value,
                        ControllerMode::SetRumble => {
                            let ret = self.rumble_config[3];
                            self.rumble_config[3] = inp;
                            ret
                        }
                        _ => 0,
                    };
                    self.state = 7;
                    (ret, false)
                }
                7 => {
                    let ret = match self.current_mode {
                        ControllerMode::GetLed => 1,
                        ControllerMode::GetWhateverValues => 1,
                        ControllerMode::GetVariableResponseA => match self.cache_value {
                            1 => 1,
                            _ => 0,
                        },
                        ControllerMode::SetRumble => {
                            let ret = self.rumble_config[4];
                            self.rumble_config[4] = inp;
                            ret
                        }
                        ControllerMode::Unknown4010 => 1,
                        _ => 0,
                    };
                    self.state = 8;
                    (ret, false)
                }
                8 => {
                    let ret = match self.current_mode {
                        ControllerMode::GetVariableResponseA => match self.cache_value {
                            0 => 0x0a,
                            1 => 0x14,
                            _ => 0,
                        },
                        ControllerMode::SetRumble => {
                            let ret = self.rumble_config[5];
                            self.rumble_config[5] = inp;
                            ret
                        }
                        ControllerMode::Config => {
                            self.in_config = self.cache_value == 1;
                            0
                        }
                        _ => 0,
                    };
                    self.state = 0;
                    (ret, true)
                }
                _ => unreachable!(),
            }
        }

        pub fn exchange_bytes(&mut self, inp: u8) -> (u8, bool) {
            let state = self.state;
            if self.in_config {
                let r = self.exchange_bytes_config(inp);
                log::trace!(
                    "C Config: State {state:?}: {:02X} -> ({:02X}, {})",
                    inp,
                    r.0,
                    r.1
                );
                r
            } else {
                let r = self.exchange_bytes_normal(inp);
                log::trace!(
                    "C Normal: State {state:?}: {:02X} -> ({:02X}, {})",
                    inp,
                    r.0,
                    r.1
                );
                r
            }
        }
    }
}

mod memcard {
    use std::{fmt::Write, fs};

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum CardReadStage {
        Command,
        MemoryCardId1,
        MemoryCardId2,
        SendAddressMsb,
        SendAddressLsb,
        ConfirmAddressMsb,
        ConfirmAddressLsb,
        Data,
        Checksum,
        CommandAck1,
        CommandAck2,
        End,

        CmdIdEnd1,
        CmdIdEnd2,
        CmdIdEnd3,
        CmdIdEnd4,
    }

    pub enum CardCmd {
        Read,
        Write,
        Id,
        Invalid,
    }

    pub struct MemoryCard {
        id: u8,
        stage: CardReadStage,
        cmd: CardCmd,
        flag: u8,
        address: u16,
        read_pointer: u8,
        checksum: u8,
        status: u8,
        previous: u8,
        data: Box<[u8; 0x400 * 128]>,
    }

    impl MemoryCard {
        pub fn new(id: u8) -> Self {
            let mut data = Box::new([0; 0x400 * 128]);

            let block0 = &mut data[0..0x400 * 8];

            block0[0] = b'M';
            block0[1] = b'C';
            block0[0x7F] = 0xE;

            // TODO: move to managed folder with resources
            fs::read(format!("memcard{}.mcd", id))
                .map(|m| {
                    if m.len() == 0x400 * 128 {
                        println!("Loaded memory card {}", id);
                        data.copy_from_slice(&m);
                    }
                })
                .ok();

            Self {
                id,
                stage: CardReadStage::Command,
                cmd: CardCmd::Read, // anything for now, will be overridden on cmd start
                flag: 0x08,
                read_pointer: 0,
                address: 0,
                checksum: 0,
                status: 0,
                previous: 0,
                data,
            }
        }

        pub fn start_access(&mut self) -> u8 {
            log::trace!("Memory card {} started access", self.id);
            self.stage = CardReadStage::Command;
            self.read_pointer = 0;
            self.address = 0;
            self.checksum = 0;
            self.status = 0;
            self.previous = 0;
            0
        }

        pub fn exchange_bytes(&mut self, inp: u8) -> (u8, bool) {
            let r = match self.stage {
                CardReadStage::Command => {
                    self.cmd = match inp {
                        b'R' => CardCmd::Read,
                        b'W' => CardCmd::Write,
                        b'S' => CardCmd::Id,
                        _ => CardCmd::Invalid, // will abort after next byte
                    };
                    self.stage = CardReadStage::MemoryCardId1;
                    (self.flag, false)
                }
                CardReadStage::MemoryCardId1 => {
                    assert_eq!(inp, 0);
                    self.stage = CardReadStage::MemoryCardId2;
                    // In case of invalid command, the communication is aborted
                    // with 0xFF
                    match self.cmd {
                        CardCmd::Invalid => {
                            self.stage = CardReadStage::Command;
                            (0xFF, true)
                        }
                        _ => (0x5A, false),
                    }
                }
                CardReadStage::MemoryCardId2 => {
                    assert_eq!(inp, 0);
                    match self.cmd {
                        CardCmd::Read | CardCmd::Write => {
                            self.stage = CardReadStage::SendAddressMsb;
                        }
                        CardCmd::Id => {
                            self.stage = CardReadStage::CommandAck1;
                        }
                        CardCmd::Invalid => unreachable!(),
                    }

                    (0x5D, false)
                }
                CardReadStage::SendAddressMsb => {
                    // start of checksum
                    self.checksum = inp;
                    self.previous = inp;
                    self.address = (inp as u16) << 8;
                    self.stage = CardReadStage::SendAddressLsb;
                    (self.previous, false)
                }
                CardReadStage::SendAddressLsb => {
                    self.checksum ^= inp;
                    self.address |= inp as u16;
                    match self.cmd {
                        CardCmd::Read => {
                            self.stage = CardReadStage::CommandAck1;
                        }
                        CardCmd::Write => {
                            self.stage = CardReadStage::Data;
                        }
                        _ => unreachable!("Id command cannot send Address"),
                    }

                    // invalid address
                    if (self.address & !0x3FF) != 0 {
                        self.status = 0xFF;
                    }

                    (self.previous, false)
                }
                CardReadStage::ConfirmAddressMsb => {
                    assert_eq!(inp, 0);

                    self.stage = CardReadStage::ConfirmAddressLsb;
                    // invalid address
                    if self.status == 0xFF {
                        (0xFF, false)
                    } else {
                        ((self.address >> 8) as u8, false)
                    }
                }
                CardReadStage::ConfirmAddressLsb => {
                    assert_eq!(inp, 0);
                    // invalid address
                    if self.status == 0xFF {
                        self.stage = CardReadStage::Command;
                        // abort transfer for Read commands
                        (0xFF, true)
                    } else {
                        self.stage = CardReadStage::Data;
                        (self.address as u8, false)
                    }
                }
                CardReadStage::Data => {
                    let r = match self.cmd {
                        CardCmd::Read => {
                            assert_eq!(inp, 0);
                            let addr = self.address as usize * 128 + self.read_pointer as usize;
                            let data = self.data[addr];
                            self.checksum ^= data;
                            data
                        }
                        CardCmd::Write => {
                            if self.read_pointer == 0 {
                                // reset flag on write
                                self.flag = 0x00;
                            }
                            // valid address
                            if self.status != 0xFF {
                                self.checksum ^= inp;
                                let addr = self.address as usize * 128 + self.read_pointer as usize;
                                self.data[addr] = inp;
                            }
                            // return previous and set it
                            std::mem::replace(&mut self.previous, inp)
                        }
                        _ => unreachable!("Id command cannot send/recv Data"),
                    };

                    self.read_pointer += 1;

                    // for debugging
                    if self.read_pointer == 128 {
                        let mut buf = String::new();
                        for i in 0..128 {
                            let addr = self.address as usize * 128 + i as usize;
                            let data = self.data[addr];
                            write!(buf, "{:02X} ", data).unwrap();
                        }
                        log::info!(
                            "[{}]: address: 0x{:04X}\n {}",
                            if let CardCmd::Read = self.cmd {
                                'R'
                            } else {
                                'W'
                            },
                            self.address,
                            buf
                        );
                        self.stage = CardReadStage::Checksum;
                    }

                    (r, false)
                }
                CardReadStage::Checksum => match self.cmd {
                    CardCmd::Read => {
                        assert_eq!(inp, 0);
                        self.stage = CardReadStage::End;
                        self.status = 0x47; // Good
                        (self.checksum, false)
                    }
                    CardCmd::Write => {
                        if self.status == 0 {
                            if self.checksum == inp {
                                self.status = 0x47; // Good
                            } else {
                                self.status = 0x4E; // Bad checksum
                            }
                        } else {
                            self.status = 0xFF;
                        }
                        self.stage = CardReadStage::CommandAck1;
                        (self.previous, false)
                    }
                    _ => unreachable!("Id command cannot send/recv Checksum"),
                },
                CardReadStage::CommandAck1 => {
                    assert_eq!(inp, 0);
                    // late /ACK after this byte-pair on Read command
                    self.stage = CardReadStage::CommandAck2;
                    (0x5C, false)
                }
                CardReadStage::CommandAck2 => {
                    assert_eq!(inp, 0);
                    match self.cmd {
                        CardCmd::Read => {
                            self.stage = CardReadStage::ConfirmAddressMsb;
                        }
                        CardCmd::Write => {
                            self.stage = CardReadStage::End;
                        }
                        CardCmd::Id => {
                            self.stage = CardReadStage::CmdIdEnd1;
                        }
                        CardCmd::Invalid => unreachable!(),
                    }

                    (0x5D, false)
                }
                CardReadStage::End => {
                    assert_eq!(inp, 0);

                    // if we finished a write command successfully, flush it to disk.
                    if let CardCmd::Write = self.cmd {
                        self.flush();
                    }

                    self.stage = CardReadStage::Command;
                    (0x4 | self.status, true)
                }
                CardReadStage::CmdIdEnd1 => {
                    assert_eq!(inp, 0);
                    self.stage = CardReadStage::CmdIdEnd2;
                    (0x04, false)
                }
                CardReadStage::CmdIdEnd2 => {
                    assert_eq!(inp, 0);
                    self.stage = CardReadStage::CmdIdEnd3;
                    (0x00, false)
                }
                CardReadStage::CmdIdEnd3 => {
                    assert_eq!(inp, 0);
                    self.stage = CardReadStage::CmdIdEnd4;
                    (0x00, false)
                }
                CardReadStage::CmdIdEnd4 => {
                    assert_eq!(inp, 0);
                    self.stage = CardReadStage::Command;
                    (0x80, true)
                }
            };

            log::trace!(
                "M{}: Stage {:?}: {:02X} -> ({:02X}, {})",
                self.id,
                self.stage,
                inp,
                r.0,
                r.1
            );
            r
        }

        /// Saves the data to disk
        fn flush(&mut self) {
            fs::write(format!("memcard{}.mcd", self.id), &self.data[..]).unwrap();
        }
    }
}

/// Groups the controller and memory_card components for communication
struct CommunicationHandler {
    /// which component we are communicating with now
    state: u8,
    controller: controller::Controller,
    memory_card: memcard::MemoryCard,
}

impl CommunicationHandler {
    /// `id` is used to indicate which memory card file to save/load from
    fn new(id: u8, controller_connected: bool) -> Self {
        Self {
            state: 0,
            controller: controller::Controller::new(controller_connected),
            memory_card: memcard::MemoryCard::new(id),
        }
    }
}

impl CommunicationHandler {
    fn exchange_bytes(&mut self, inp: u8) -> u8 {
        match self.state {
            0 => match inp {
                0x01 => {
                    let out = self.controller.start_access();
                    if out != 0xFF {
                        self.state = 1;
                    }
                    out
                }
                0x81 => {
                    let out = self.memory_card.start_access();
                    if out != 0xFF {
                        self.state = 2;
                    }
                    out
                }
                _ => {
                    log::warn!("Invalid first received: 0x{:02X}", inp);
                    self.state = 0;
                    0
                }
            },
            1 => {
                let (result, done) = self.controller.exchange_bytes(inp);
                if done {
                    self.state = 0;
                }
                result
            }
            2 => {
                let (result, done) = self.memory_card.exchange_bytes(inp);
                if done {
                    self.state = 0;
                }
                result
            }
            _ => unreachable!(),
        }
    }

    fn change_controller_key_state(&mut self, key: DigitalControllerKey, pressed: bool) {
        self.controller.change_key_state(key, pressed);
    }

    fn has_more(&self) -> bool {
        self.state != 0
    }
}

pub struct ControllerAndMemoryCard {
    ctrl: JoyControl,
    mode: JoyMode,
    stat: JoyStat,
    baudrate_timer_reload: u32,
    baudrate_timer: u32,
    clk_position_high: bool,
    transfered_bits: u8,
    tx_fifo: VecDeque<u8>,
    rx_fifo: VecDeque<u8>,

    communication_handlers: [CommunicationHandler; 2],
}

impl Default for ControllerAndMemoryCard {
    fn default() -> Self {
        let baudrate_timer_reload = 0x0088;
        let baudrate_timer = baudrate_timer_reload / 2;
        Self {
            ctrl: JoyControl::empty(),
            mode: JoyMode::from_bits_retain(0x000D),
            stat: JoyStat::TX_READY_1 | JoyStat::TX_READY_2,
            baudrate_timer_reload,
            baudrate_timer,
            transfered_bits: 0,
            clk_position_high: false,
            tx_fifo: VecDeque::new(),
            rx_fifo: VecDeque::new(),

            communication_handlers: [
                CommunicationHandler::new(0, true),
                CommunicationHandler::new(1, false),
            ],
        }
    }
}

impl ControllerAndMemoryCard {
    pub fn clock(&mut self, interrupt_requester: &mut impl InterruptRequester, mut cycles: u32) {
        while cycles > 0 {
            let (r, overflow) = cycles.overflowing_sub(self.baudrate_timer);

            if overflow {
                // this cannot overflow, because the `self.baudrate_timer` is
                // larger than `cycles`, so no need to check
                self.baudrate_timer -= cycles;
                return;
            }

            cycles = r;
            // reset to zero (as if we subtracted from `cycles`)
            self.baudrate_timer = 0;

            // reload timer
            self.trigger_baudrate_reload();
            // advance the clock one step
            self.clk_position_high = !self.clk_position_high;

            // if there is anything to transfer and
            // the clock changes from idle to active, transfer one bit
            if !self.tx_fifo.is_empty()
                && self.ctrl.tx_enable()
                && (self.clk_position_high ^ self.mode.clk_idle_on_high())
            {
                self.transfered_bits += 1;

                if self.transfered_bits == self.mode.character_length() {
                    self.transfered_bits = 0;
                    let byte_to_send = self.tx_fifo.pop_front().unwrap();
                    let byte_mask = 0xFF >> (8 - self.mode.character_length());
                    // make sure we don't have extra stuff to send and we are not sending
                    assert!(byte_to_send & !byte_mask == 0);
                    log::info!("sending byte {:02X}", byte_to_send);

                    let slot = self.ctrl.joy_slot() as usize;

                    let received_byte =
                        self.communication_handlers[slot].exchange_bytes(byte_to_send);
                    // make sure we don't have extra stuff to send and we are not sending
                    assert!(received_byte & !byte_mask == 0);
                    log::info!("got byte {:02X}", received_byte);
                    if self.ctrl.joy_selected() || self.ctrl.rx_force_enable() {
                        self.push_to_rx_fifo(received_byte);
                    }

                    if self.communication_handlers[slot].has_more() {
                        self.send_ack_interrupt();
                        interrupt_requester.request_controller_mem_card();
                    }
                }
            }
        }
    }

    pub fn change_controller_key_state(&mut self, key: DigitalControllerKey, pressed: bool) {
        self.communication_handlers[0].change_controller_key_state(key, pressed);
    }
}

impl ControllerAndMemoryCard {
    fn get_stat(&self) -> u32 {
        let timer = self.baudrate_timer & 0x1FFFFF;
        self.stat.bits() | (timer << 11)
    }

    fn trigger_baudrate_reload(&mut self) {
        let factored_reload =
            self.baudrate_timer_reload << self.mode.baudrate_reload_factor_shift();
        self.baudrate_timer = factored_reload / 2;
    }

    fn push_to_tx_fifo(&mut self, data: u8) {
        // max size is 2
        assert!(self.tx_fifo.len() < 2);
        self.tx_fifo.push_back(data);
    }

    fn push_to_rx_fifo(&mut self, data: u8) {
        // max size is 8
        assert!(self.tx_fifo.len() < 8);
        self.rx_fifo.push_back(data);
        self.stat.insert(JoyStat::RX_FIFO_NOT_EMPTY);
    }

    fn pop_from_rx_fifo(&mut self) -> u8 {
        let out = self.rx_fifo.pop_front().unwrap_or(0);
        if self.rx_fifo.is_empty() {
            self.stat.remove(JoyStat::RX_FIFO_NOT_EMPTY);
        }
        out
    }

    fn send_ack_interrupt(&mut self) {
        // self.stat.insert(JoyStat::ACK_INPUT_LEVEL_LOW);
        if self.ctrl.ack_interrupt_enable() {
            self.stat.insert(JoyStat::INTERRUPT_REQUEST);
        }
    }

    fn acknowledge_interrupt(&mut self) {
        self.stat
            .remove(JoyStat::INTERRUPT_REQUEST | JoyStat::RX_PARITY_ERROR);
    }
}

impl BusLine for ControllerAndMemoryCard {
    fn read_u32(&mut self, addr: u32) -> Result<u32> {
        let r = match addr {
            0x4 => self.get_stat(),
            _ => unreachable!(),
        };
        Ok(r)
    }

    fn read_u16(&mut self, addr: u32) -> Result<u16> {
        let r = match addr {
            0x4 => self.get_stat() as u16,
            0x8 => self.mode.bits(),
            0xA => self.ctrl.bits(),
            0xE => self.baudrate_timer_reload as u16,
            _ => unreachable!(),
        };
        Ok(r)
    }

    fn write_u16(&mut self, addr: u32, data: u16) -> Result<()> {
        match addr {
            0x8 => {
                self.mode = JoyMode::from_bits_retain(data);
                log::info!("joy mode write {:04X} => {:?}", data, self.mode);
            }
            0xA => {
                self.ctrl = JoyControl::from_bits_retain(data);
                log::info!("joy ctrl write {:04X} => {:?}", data, self.ctrl);
                if data & JOY_CTRL_ACKKNOWLEDGE != 0 {
                    log::info!("joy acknowledge interrupt");
                    self.acknowledge_interrupt();
                }
                if data & JOY_CTRL_RESET != 0 {
                    // TODO: not sure what it means by "Reset most JOY_registers to zero"
                    log::info!("joy reset");

                    self.trigger_baudrate_reload();
                    self.transfered_bits = 0;
                    self.tx_fifo.clear();
                    self.rx_fifo.clear();
                    self.clk_position_high = false;

                    // reset the communication handlers
                    self.communication_handlers[0].state = 0;
                    self.communication_handlers[1].state = 0;
                }

                // zero, reset communication handlers
                if data == 0 {
                    self.trigger_baudrate_reload();
                    self.transfered_bits = 0;
                    self.tx_fifo.clear();
                    self.rx_fifo.clear();
                    self.clk_position_high = false;

                    self.communication_handlers[0].state = 0;
                    self.communication_handlers[1].state = 0;
                }
            }
            0xE => {
                log::info!("baudrate reload value {:04X}", data);
                self.baudrate_timer_reload = data as u32;
                self.trigger_baudrate_reload();
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    // only used with 0x1F801040
    fn read_u8(&mut self, addr: u32) -> Result<u8> {
        assert!(addr == 0);
        Ok(self.pop_from_rx_fifo())
    }

    // only used with 0x1F801040
    fn write_u8(&mut self, addr: u32, data: u8) -> Result<()> {
        assert!(addr == 0);
        self.push_to_tx_fifo(data);
        log::info!("Add to TX fifo {:02X}", data);
        Ok(())
    }
}
