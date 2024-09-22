use crate::{memory::Result, PsxConfig};

use super::BusLine;

// for now there is no external device to be hooked in PIO extension, so maybe
// use it as ram?
//
// FIXME: check what is the behaviour of expansion regions 1, 2, 3 in the case
//  that there is no device connected.
//
// For now using as ram
pub struct ExpansionRegion1 {
    data: Box<[u8; 0x80000]>,
}

impl Default for ExpansionRegion1 {
    fn default() -> Self {
        Self {
            data: Box::new([0; 0x80000]),
        }
    }
}

impl BusLine for ExpansionRegion1 {
    fn read_u8(&mut self, addr: u32) -> Result<u8> {
        Ok(self.data[addr as usize])
    }

    fn write_u8(&mut self, addr: u32, data: u8) -> Result<()> {
        self.data[addr as usize] = data;

        log::info!(
            "expansion region r1 write at {:05X}, value {:02X}",
            addr,
            data
        );
        Ok(())
    }
}

struct DuartTTY {
    // TODO: add a way to display this buffer, maybe using another window?
    tty_buffer: String,
    line_temp_buffer: String,
    config: PsxConfig,
}

// This is just the minimum for the TTY to work, as the duart is not used
// for anything else
impl DuartTTY {
    fn new(config: PsxConfig) -> Self {
        Self {
            tty_buffer: String::new(),
            line_temp_buffer: String::new(),
            config,
        }
    }

    fn read(&self, addr: u32) -> u8 {
        match addr & 0xF {
            0x0 => todo!(),
            // DUART Status Register A
            // bit.2: Tx Empty (ready to send)
            0x1 => 0b100,
            0x2 => todo!(),
            0x3 => todo!(),
            0x4 => todo!(),
            0x5 => todo!(),
            0x6 => todo!(),
            0x7 => todo!(),
            0x8 => todo!(),
            0x9 => todo!(),
            0xA => todo!(),
            0xB => todo!(),
            0xC => todo!(),
            0xD => todo!(),
            0xE => todo!(),
            0xF => todo!(),
            _ => unreachable!(),
        }
    }

    fn write(&mut self, addr: u32, data: u8) {
        match addr & 0xF {
            // DUART Mode Register A
            0x0 => {}
            // DUART Clock Select Register A
            0x1 => {}
            // DUART Command Register A
            // used for clearing errors, enabling and disabling Rx and Tx
            0x2 => {}
            // DUART Tx Holding Register A, sending characters through
            0x3 => {
                let ch = data as char;
                self.tty_buffer.push(ch);

                // printing each line on line break to not get mixed with logs
                if ch == '\n' {
                    if self.config.stdout_debug {
                        println!("DEBUG: {}", self.line_temp_buffer);
                    }
                    self.line_temp_buffer.clear();
                } else {
                    self.line_temp_buffer.push(ch);
                }
            }
            // DUART Aux. Control Register
            0x4 => {}
            // DUART Interrupt Mask Register
            // 0 is written here, so no need to handle any interrupts
            0x5 => {}
            0x6 => todo!(),
            0x7 => todo!(),
            0x8 => todo!(),
            0x9 => todo!(),
            // DUART Command Register B
            0xA => {}
            0xB => todo!(),
            0xC => todo!(),
            // DUART Output Port Configuration Register
            0xD => {}
            // DUART Set Output Port Bits Command
            0xE => {}
            0xF => todo!(),
            _ => unreachable!(),
        }
    }
}

pub struct ExpansionRegion2 {
    // the original size is 0x80, but pcsx-redux uses some of the space after it
    // for its own purposes, we don't support it, but at least no reason to give exceptions
    data: [u8; 0x90],
    tty_duart: DuartTTY,
}

impl ExpansionRegion2 {
    pub fn new(config: PsxConfig) -> Self {
        Self {
            data: [0; 0x90],
            tty_duart: DuartTTY::new(config),
        }
    }
}

impl BusLine for ExpansionRegion2 {
    fn read_u8(&mut self, addr: u32) -> Result<u8> {
        let out = match addr {
            0x20..=0x2F => self.tty_duart.read(addr & 0xF),
            _ => self.data[addr as usize],
        };

        log::info!("expansion region 2 read at {:02X}, value {:02X}", addr, out);
        Ok(out)
    }

    fn write_u8(&mut self, addr: u32, data: u8) -> Result<()> {
        log::info!(
            "expansion region 2 write at {:02X}, value {:02X}",
            addr,
            data
        );

        match addr {
            0x20..=0x2F => self.tty_duart.write(addr & 0xF, data),
            // POST register used for debugging the BIOS and kernel init
            0x41 => println!("TraceStep {:02X}", data),
            _ => self.data[addr as usize] = data,
        }

        self.data[addr as usize] = data;
        Ok(())
    }

    fn read_u16(&mut self, addr: u32) -> Result<u16> {
        let low = self.read_u8(addr)? as u16;
        let high = self.read_u8(addr + 1)? as u16;

        Ok(low | (high << 8))
    }

    fn write_u16(&mut self, addr: u32, data: u16) -> Result<()> {
        self.write_u8(addr, data as u8)?;
        self.write_u8(addr + 1, (data >> 8) as u8)
    }

    fn read_u32(&mut self, addr: u32) -> Result<u32> {
        let low = self.read_u16(addr)? as u32;
        let high = self.read_u16(addr + 2)? as u32;

        Ok(low | (high << 16))
    }

    fn write_u32(&mut self, addr: u32, data: u32) -> Result<()> {
        self.write_u16(addr, data as u16)?;
        self.write_u16(addr + 2, (data >> 16) as u16)
    }
}
