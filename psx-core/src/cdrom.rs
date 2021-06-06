use crate::memory::{interrupts::InterruptRequester, BusLine};
use bitflags::bitflags;

use std::collections::VecDeque;

bitflags! {
    #[derive(Default)]
    struct CdromStatus: u8 {
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

pub struct Cdrom {
    index: u8,
    status: CdromStatus,
    interrupt_enable: u8,
    interrupt_flag: u8,
    parameter_fifo: VecDeque<u8>,
    response_fifo: VecDeque<u8>,
    command: Option<u8>,
}

impl Default for Cdrom {
    fn default() -> Self {
        Self {
            index: 0,
            status: CdromStatus::PARAMETER_FIFO_EMPTY | CdromStatus::PARAMETER_FIFO_NOT_FULL,
            interrupt_enable: 0,
            interrupt_flag: 0,
            parameter_fifo: VecDeque::new(),
            response_fifo: VecDeque::new(),
            command: None,
        }
    }
}

impl Cdrom {
    pub fn clock(&mut self, interrupt_requester: &mut impl InterruptRequester) {
        self.execute_next_command(interrupt_requester);
    }

    fn execute_next_command(&mut self, interrupt_requester: &mut impl InterruptRequester) {
        if let Some(cmd) = self.command.take() {
            match cmd {
                0x19 => {
                    let test_code = self.read_next_parameter().unwrap();
                    self.execute_test(test_code);
                    interrupt_requester.request_cdrom();
                    // not executing any more
                    self.status.remove(CdromStatus::BUSY);
                }
                _ => todo!(),
            }
        }
    }

    fn execute_test(&mut self, test_code: u8) {
        match test_code {
            0x20 => {
                let data_version = &[0x99u8, 0x02, 0x01, 0xC3];
                for v in data_version {
                    self.write_to_response_fifo(*v);
                }
                self.request_interrupt_0_7(3);
            }
            _ => todo!(),
        }
    }
}

impl Cdrom {
    fn read_index_status(&self) -> u8 {
        self.index | self.status.bits
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
        // set the status to busy as we are sending/executing a command now
        self.status.insert(CdromStatus::BUSY);
        self.command = Some(data);
    }

    fn reset_parameter_fifo(&mut self) {
        self.status.insert(CdromStatus::PARAMETER_FIFO_EMPTY);
        self.status.insert(CdromStatus::PARAMETER_FIFO_NOT_FULL);
        self.parameter_fifo.clear();
    }

    fn write_to_parameter_fifo(&mut self, data: u8) {
        if self.parameter_fifo.len() == 0 {
            self.status.remove(CdromStatus::PARAMETER_FIFO_EMPTY);
        } else if self.parameter_fifo.len() == 15 {
            self.status.remove(CdromStatus::PARAMETER_FIFO_NOT_FULL);
        }
        log::info!("2.0 writing to parameter fifo={:02X}", data);

        self.parameter_fifo.push_back(data);
    }

    fn read_next_parameter(&mut self) -> Option<u8> {
        let out = self.parameter_fifo.pop_front();
        if self.parameter_fifo.len() == 0 {
            self.status.insert(CdromStatus::PARAMETER_FIFO_EMPTY);
        } else if self.parameter_fifo.len() == 15 {
            self.status.insert(CdromStatus::PARAMETER_FIFO_NOT_FULL);
        }

        out
    }

    fn write_to_response_fifo(&mut self, data: u8) {
        if self.response_fifo.is_empty() {
            self.status.insert(CdromStatus::RESPONSE_FIFO_NOT_EMPTY);
        }
        log::info!("writing to response fifo={:02X}", data);

        self.response_fifo.push_back(data);
    }

    fn read_next_response(&mut self) -> u8 {
        let out = self.response_fifo.pop_front();

        if self.response_fifo.is_empty() {
            self.status.remove(CdromStatus::RESPONSE_FIFO_NOT_EMPTY);
        }

        // TODO: handle reading while being empty
        out.unwrap()
    }

    fn request_interrupt_0_7(&mut self, int_value: u8) {
        let int_value = int_value & 0x7;

        if self.interrupt_enable & int_value == int_value {
            self.interrupt_flag &= !7;
            self.interrupt_flag |= int_value;
        }
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
            2 => todo!("read 2"),
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
                log::info!("set index {}", self.index);
            }
            1 => match self.index {
                0 => self.write_command_register(data),
                1 => todo!("write 1.1 (unknown/unused)"),
                2 => todo!("write 1.2 (unknown/unused)"),
                3 => todo!("write 1.3 vol stuff"),
                _ => unreachable!(),
            },
            2 => match self.index {
                0 => self.write_to_parameter_fifo(data),
                1 => self.write_interrupt_enable_register(data),
                2 => todo!("write 2.2 vol stuff"),
                3 => todo!("write 2.3 vol stuff"),
                _ => unreachable!(),
            },
            3 => match self.index {
                0 => todo!("write 3.0 request register"),
                1 => self.write_interrupt_flag_register(data),
                2 => todo!("write 3.2 vol stuff"),
                3 => todo!("write 3.3 vol stuff"),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }
}
