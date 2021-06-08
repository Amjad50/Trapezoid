use crate::memory::{interrupts::InterruptRequester, BusLine};
use bitflags::bitflags;

use std::collections::VecDeque;

const JOY_CTRL_ACKKNOWLEDGE: u16 = 0b0000000000010000;
const JOY_CTRL_RESET: u16 = 0b0000000001000000;
bitflags! {
    #[derive(Default)]
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
    #[derive(Default)]
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
        let bits = (self.bits & Self::BAUDRATE_RELOAD_FACTOR.bits) as u32;

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
        let bits = ((self.bits & Self::CHARACTER_LENGTH.bits) >> 2) as u8;

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

/// Emulate Digital pad controller communication
/// and TODO: memory card
struct CommunicationHandler {
    // handles which byte we should send next
    state: u8,
    device_id: u16,
    digital_switches: u16,
    connected: bool,
}

impl CommunicationHandler {
    fn new(connected: bool) -> Self {
        Self {
            state: 0,
            device_id: 0x5A41,
            digital_switches: 0,
            connected,
        }
    }
}

impl CommunicationHandler {
    fn exchange_bytes(&mut self, inp: u8) -> u8 {
        // controller not connected results in floating bus
        if !self.connected {
            return 0xFF;
        }

        match self.state {
            0 => {
                assert_eq!(inp, 0x01);
                self.state = 1;
                // garbage
                0
            }
            1 => {
                assert_eq!(inp, 0x42);
                self.state = 2;
                (self.device_id & 0xFF) as u8
            }
            2 => {
                // TODO: handle mutlitap support
                assert_eq!(inp, 0);
                self.state = 3;
                ((self.device_id >> 8) & 0xFF) as u8
            }
            3 => {
                // TODO: handle rumble support
                assert_eq!(inp, 0);
                self.state = 4;
                (self.digital_switches & 0xFF) as u8
            }
            4 => {
                // TODO: handle rumble support
                assert_eq!(inp, 0);
                self.state = 0;
                ((self.digital_switches >> 8) & 0xFF) as u8
            }
            _ => unreachable!(),
        }
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
        Self {
            ctrl: JoyControl::empty(),
            mode: JoyMode::empty(),
            stat: JoyStat::TX_READY_1 | JoyStat::TX_READY_2,
            baudrate_timer_reload: 0,
            baudrate_timer: 0,
            transfered_bits: 0,
            clk_position_high: false,
            tx_fifo: VecDeque::new(),
            rx_fifo: VecDeque::new(),

            communication_handlers: [
                CommunicationHandler::new(false),
                CommunicationHandler::new(false),
            ],
        }
    }
}

impl ControllerAndMemoryCard {
    pub fn clock(&mut self, interrupt_requester: &mut impl InterruptRequester) {
        self.baudrate_timer = self.baudrate_timer.saturating_sub(1);

        if self.baudrate_timer == 0 {
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
}

impl ControllerAndMemoryCard {
    fn get_stat(&self) -> u32 {
        let timer = self.baudrate_timer & 0x1FFFFF;
        self.stat.bits | (timer << 11)
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
    fn read_u32(&mut self, addr: u32) -> u32 {
        todo!()
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        todo!()
    }

    fn read_u16(&mut self, addr: u32) -> u16 {
        match addr {
            0x4 => self.get_stat() as u16,
            0x8 => todo!("read 8"),
            0xA => self.ctrl.bits,
            0xE => self.baudrate_timer_reload as u16,
            _ => unreachable!(),
        }
    }

    fn write_u16(&mut self, addr: u32, data: u16) {
        match addr {
            0x8 => {
                self.mode = JoyMode::from_bits_truncate(data);
                log::info!("joy mode write {:04X} => {:?}", data, self.mode);
            }
            0xA => {
                self.ctrl = JoyControl::from_bits_truncate(data);
                log::info!("joy ctrl write {:04X} => {:?}", data, self.ctrl);
                if data & JOY_CTRL_ACKKNOWLEDGE != 0 {
                    log::info!("joy acknowledge interrupt");
                    self.acknowledge_interrupt();
                }
                if data & JOY_CTRL_RESET != 0 {
                    // TODO: not sure what it means by "Reset most JOY_registers to zero"
                    log::info!("joy reset");
                }
            }
            0xE => {
                log::info!("baudrate reload value {:04X}", data);
                self.baudrate_timer_reload = data as u32;
                self.trigger_baudrate_reload();
            }
            _ => unreachable!(),
        }
    }

    // only used with 0x1F801040
    fn read_u8(&mut self, addr: u32) -> u8 {
        assert!(addr == 0);
        self.pop_from_rx_fifo()
    }

    // only used with 0x1F801040
    fn write_u8(&mut self, addr: u32, data: u8) {
        assert!(addr == 0);
        self.push_to_tx_fifo(data);
        log::info!("Add to TX fifo {:02X}", data);
    }
}
