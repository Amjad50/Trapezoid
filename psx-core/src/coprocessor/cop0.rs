#[derive(Default)]
pub struct SystemControlCoprocessor {
    bpc: u32,
    bda: u32,
    jmp_dest: u32,
    dcic: u32,
    bad_vaddr: u32,
    bdam: u32,
    bpcm: u32,
    sr: u32,
    cause: u32,
    epc: u32,
    prid: u32,
}

impl SystemControlCoprocessor {
    pub fn is_cache_isolated(&self) -> bool {
        self.sr & 0x10000 != 0
    }

    pub fn read_cause(&self) -> u32 {
        self.cause
    }

    pub fn write_cause(&mut self, data: u32) {
        self.cause = data;
    }

    pub fn read_sr(&self) -> u32 {
        self.sr
    }

    pub fn write_sr(&mut self, data: u32) {
        self.sr = data;
    }

    pub fn write_epc(&mut self, data: u32) {
        self.epc = data;
    }
}

impl SystemControlCoprocessor {
    pub fn read_ctrl(&self, num: u8) -> u32 {
        assert!(num <= 0x1F);
        // no control registers
        todo!("cop0 ctrl read {}", num)
    }

    pub fn write_ctrl(&mut self, num: u8, data: u32) {
        assert!(num <= 0x1F);
        // no control registers
        todo!("cop0 ctrl write {}, data={:08X}", num, data)
    }

    pub fn read_data(&self, num: u8) -> u32 {
        assert!(num <= 0x1F);

        log::info!("COP0 data_read {}", num);
        match num {
            // FIXME: reading any of these causes reserved instruction exception
            //0..=2 | 4 | 10 => 0, // N/A
            //3 => self.bpc,
            //5 => self.bda,
            //6 => self.jmp_dest,
            //7 => self.dcic,
            //8 => self.bad_vaddr,
            //9 => self.bdam,
            //11 => self.bpcm,
            12 => self.sr,
            13 => self.cause,
            14 => self.epc,
            //15 => self.prid,
            // When reading one of the garbage registers shortly after reading
            // a valid cop0 register, the garbage value is usually the same
            // as that of the valid register. When doing the read later on,
            // the return value is usually 00000020h, or when reading much
            // later it returns 00000040h, or even 00000100h.
            16..=31 => 0xFF,
            0..=15 => todo!("cop0 read data {}", num),
            _ => unreachable!(),
        }
    }

    pub fn write_data(&mut self, num: u8, data: u32) {
        assert!(num <= 0x1F);

        log::info!("COP0 data_write {}, data={:08X}", num, data);
        match num {
            // FIXME: does writing produce reserved instruction exception?
            //0..=2 | 4 | 10 => {}  // N/A
            3 => self.bpc = data,
            5 => self.bda = data,
            6 => {}
            7 => self.dcic = data,
            // 8 => {}
            9 => self.bdam = data,
            11 => self.bpcm = data,
            12 => self.sr = data,
            13 => {
                self.cause &= !0x300;
                self.cause |= data & 0x300;
            }
            //14 => {}
            //15 -> {}
            16..=31 => {} // garbage
            0..=15 => todo!("cop0 write data {}, vaule {:08X}", num, data),
            _ => unreachable!(),
        }
    }
}
