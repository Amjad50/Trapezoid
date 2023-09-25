use crate::memory::{interrupts::InterruptRequester, BusLine, Result};
use bitflags::bitflags;

bitflags! {
    #[derive(Default, Debug)]
    struct CounterMode: u16 {
        const SYNC_ENABLE        = 0b0000000000000001;
        const SYNC_MODE          = 0b0000000000000110;
        const RESET_AFTER_TARGET = 0b0000000000001000;
        const IRQ_ON_TARGET      = 0b0000000000010000;
        const IRQ_ON_FFFF        = 0b0000000000100000;
        const IRQ_REPEAT_MODE    = 0b0000000001000000;
        const IRQ_TOGGLE_MODE    = 0b0000000010000000;
        const CLK_SOURCE         = 0b0000001100000000;
        /// inverted, (0=Yes, 1=No)
        const NOT_IRQ_REQUEST    = 0b0000010000000000;
        const REACHED_TARGET     = 0b0000100000000000;
        const REACHED_FFFF       = 0b0001000000000000;
        // const NOT_USED        = 0b1110000000000000;
    }
}

impl CounterMode {
    fn sync_enable(&self) -> bool {
        self.intersects(Self::SYNC_ENABLE)
    }

    fn sync_mode(&self) -> u8 {
        ((self.bits() & Self::SYNC_MODE.bits()) >> 1) as u8
    }

    fn clk_source(&self) -> u8 {
        ((self.bits() & Self::CLK_SOURCE.bits()) >> 8) as u8
    }

    fn reset_after_target(&self) -> bool {
        self.intersects(Self::RESET_AFTER_TARGET)
    }

    fn irq_on_target(&self) -> bool {
        self.intersects(Self::IRQ_ON_TARGET)
    }

    fn irq_on_ffff(&self) -> bool {
        self.intersects(Self::IRQ_ON_FFFF)
    }

    fn irq_repeat_mode(&self) -> bool {
        self.intersects(Self::IRQ_REPEAT_MODE)
    }

    fn irq_toggle_mode(&self) -> bool {
        self.intersects(Self::IRQ_TOGGLE_MODE)
    }

    fn set_reached_target(&mut self) {
        self.insert(Self::REACHED_TARGET)
    }

    fn set_reached_ffff(&mut self) {
        self.insert(Self::REACHED_FFFF)
    }

    fn irq(&self) -> bool {
        self.intersects(Self::NOT_IRQ_REQUEST)
    }

    fn set_irq(&mut self) {
        self.insert(Self::NOT_IRQ_REQUEST);
    }

    fn reset_irq(&mut self) {
        self.remove(Self::NOT_IRQ_REQUEST);
    }

    fn toggle_irq(&mut self) {
        self.toggle(Self::NOT_IRQ_REQUEST);
    }
}

#[derive(Default)]
struct TimerBase {
    mode: CounterMode,
    counter: u16,
    target: u16,
    paused: bool,
    one_shot_suppress_irqs: bool,
    should_request_interrupt: bool,
}

impl TimerBase {
    fn read(&mut self, index: u32) -> u16 {
        match index {
            0 => self.counter,
            1 => {
                let out = self.mode.bits();
                // reset after read
                self.mode
                    .remove(CounterMode::REACHED_FFFF | CounterMode::REACHED_TARGET);

                out
            }
            2 => self.target,
            _ => unreachable!(),
        }
    }

    fn write(&mut self, index: u32, data: u16) {
        match index {
            0 => {
                self.counter = data;
                log::info!("write current {:04X}", self.counter);
            }
            1 => {
                // reset one shot irq suppress
                self.one_shot_suppress_irqs = false;

                let mode = CounterMode::from_bits_retain(data & 0x3FF);
                if data & 0x400 != 0 {
                    // reset IRQ request
                    self.mode.insert(CounterMode::NOT_IRQ_REQUEST);
                }

                self.mode &= CounterMode::from_bits_retain(!0x3FF);
                self.mode |= mode;

                // reset on write to mode
                self.counter = 0;

                log::info!("write mode {:?}", self.mode);
            }
            2 => {
                self.target = data;
                log::info!("write target {:04X}", self.target);
            }
            _ => unreachable!(),
        }
    }

    fn increment_counter(&mut self, cycles: u32) {
        // this can happen for timer 0 and 1 in special times, like
        //  inside Hblank or Vblank
        if self.paused {
            return;
        }

        let old_irq = self.mode.irq();

        assert!(cycles <= 0xFFFF);
        let (new_counter, overflow) = self.counter.overflowing_add(cycles as u16);

        let reached_target = self.counter < self.target && new_counter >= self.target;
        self.counter = new_counter;

        let mut irq = false;
        let is_one_shot_mode = !self.mode.irq_repeat_mode();
        // there should not be irq
        let one_shot_mode_irq_supressed = is_one_shot_mode && self.one_shot_suppress_irqs;

        if reached_target {
            self.mode.set_reached_target();
            if self.mode.irq_on_target() {
                irq = true;
            }
            if self.mode.reset_after_target() {
                self.counter -= self.target;
            }
        }

        if overflow {
            self.mode.set_reached_ffff();
            if self.mode.irq_on_ffff() {
                irq = true;
            }
        }

        if irq && !one_shot_mode_irq_supressed {
            if is_one_shot_mode {
                self.one_shot_suppress_irqs = true;
            }

            if self.mode.irq_toggle_mode() {
                self.mode.toggle_irq();
            } else {
                // reset sets the bit to 0, which means interrrupt signal
                self.mode.reset_irq();
            }
        }

        let new_irq = self.mode.irq();

        // only for transition from 1 to 0
        if old_irq && !new_irq {
            self.should_request_interrupt = true;
        }

        // if its pulse mode, then set it back
        if !self.mode.irq_toggle_mode() {
            self.mode.set_irq();
        }
    }

    /// This is so that the base timer knows that it can set the irq line
    ///  in pulse mode
    fn get_irq_requested(&mut self) -> bool {
        let result = self.should_request_interrupt;
        // reset
        self.should_request_interrupt = false;
        result
    }
}

#[derive(Default)]
struct Timer0 {
    base: TimerBase,
}

impl Timer0 {
    fn mode(&mut self) -> &CounterMode {
        &self.base.mode
    }

    fn read(&mut self, index: u32) -> u16 {
        self.base.read(index)
    }

    fn write(&mut self, index: u32, data: u16) {
        self.base.write(index, data);
    }

    fn get_irq_requested(&mut self) -> bool {
        self.base.get_irq_requested()
    }
}

impl Timer0 {
    fn increment_counter(&mut self, cycles: u32) {
        let sync_mode = self.mode().sync_mode();
        if self.mode().sync_enable() {
            // TODO: fix sync modes
            self.base.increment_counter(cycles);
            match sync_mode {
                0 => {}
                1 => {}
                2 => {}
                3 => {}
                _ => unreachable!(),
            }
        } else {
            self.base.increment_counter(cycles);
        }
    }
}

#[derive(Default)]
struct Timer1 {
    base: TimerBase,
}

impl Timer1 {
    fn mode(&mut self) -> &CounterMode {
        &self.base.mode
    }

    fn read(&mut self, index: u32) -> u16 {
        self.base.read(index)
    }

    fn write(&mut self, index: u32, data: u16) {
        self.base.write(index, data);
    }

    fn get_irq_requested(&mut self) -> bool {
        self.base.get_irq_requested()
    }
}

impl Timer1 {
    fn increment_counter(&mut self, cycles: u32) {
        let sync_mode = self.mode().sync_mode();
        if self.mode().sync_enable() {
            // TODO: fix sync modes
            self.base.increment_counter(cycles);
            match sync_mode {
                0 => {}
                1 => {}
                2 => {}
                3 => {}
                _ => unreachable!(),
            }
        } else {
            self.base.increment_counter(cycles);
        }
    }
}

#[derive(Default)]
struct Timer2 {
    base: TimerBase,
    divider_counter: u32,
}

impl Timer2 {
    fn mode(&mut self) -> &CounterMode {
        &self.base.mode
    }

    fn read(&mut self, index: u32) -> u16 {
        self.base.read(index)
    }

    fn write(&mut self, index: u32, data: u16) {
        self.base.write(index, data);
    }

    fn get_irq_requested(&mut self) -> bool {
        self.base.get_irq_requested()
    }
}

impl Timer2 {
    fn increment_counter(&mut self, cycles: u32) {
        let sync_mode = self.mode().sync_mode();
        if self.mode().sync_enable() && (sync_mode == 0 || sync_mode == 3) {
            // stop counter at current value forever
        } else {
            self.base.increment_counter(cycles);
        }
    }

    fn clock_from_system(&mut self, cycles: u32) {
        // 0 or 1
        // system clock
        if self.mode().clk_source() & 2 == 0 {
            self.increment_counter(cycles);
        } else {
            self.divider_counter += cycles;
            self.increment_counter(self.divider_counter / 8);
            // reset divider
            self.divider_counter %= 8;
        }
    }
}

#[derive(Default)]
pub struct Timers {
    timer0: Timer0,
    timer1: Timer1,
    timer2: Timer2,
}

impl Timers {
    pub fn clock_from_system(&mut self, cycles: u32) {
        // 0 or 2
        if self.timer0.mode().clk_source() & 1 == 0 {
            self.timer0.increment_counter(cycles);
        }

        // 0 or 2
        if self.timer1.mode().clk_source() & 1 == 0 {
            self.timer1.increment_counter(cycles);
        }

        self.timer2.clock_from_system(cycles);
    }

    pub fn clock_from_gpu_dot(&mut self, dot_clocks: u32) {
        if self.timer0.mode().clk_source() & 1 == 1 {
            self.timer0.increment_counter(dot_clocks);
        }
    }

    pub fn clock_from_hblank(&mut self) {
        if self.timer1.mode().clk_source() & 1 == 1 {
            self.timer1.increment_counter(1);
        }
    }

    /// Request interrupts if any are queued from the previous clocking
    pub fn handle_interrupts(&mut self, interrupt_requester: &mut impl InterruptRequester) {
        if self.timer0.get_irq_requested() {
            interrupt_requester.request_timer0();
        }
        if self.timer1.get_irq_requested() {
            interrupt_requester.request_timer1();
        }
        if self.timer2.get_irq_requested() {
            interrupt_requester.request_timer2();
        }
    }

    fn read(&mut self, timer_index: u32, reg_index: u32) -> u16 {
        match timer_index {
            0 => self.timer0.read(reg_index),
            1 => self.timer1.read(reg_index),
            2 => self.timer2.read(reg_index),
            _ => unreachable!(),
        }
    }

    fn write(&mut self, timer_index: u32, reg_index: u32, data: u16) {
        match timer_index {
            0 => self.timer0.write(reg_index, data),
            1 => self.timer1.write(reg_index, data),
            2 => self.timer2.write(reg_index, data),
            _ => unreachable!(),
        }
    }
}

impl BusLine for Timers {
    fn read_u32(&mut self, addr: u32) -> Result<u32> {
        let timer_index = (addr >> 4) & 0x3;
        let reg_index = (addr & 0xF) / 4;

        Ok(self.read(timer_index, reg_index) as u32)
    }

    fn write_u32(&mut self, addr: u32, data: u32) -> Result<()> {
        let timer_index = (addr >> 4) & 0x3;
        let reg_index = (addr & 0xF) / 4;

        log::info!(
            "written timer register addr=0x{:X}, data=0x{:X}",
            addr,
            data
        );

        self.write(timer_index, reg_index, data as u16);
        Ok(())
    }

    fn read_u16(&mut self, addr: u32) -> Result<u16> {
        let timer_index = (addr >> 4) & 0x3;
        let is_inside_reg = ((addr & 0xF) / 2) % 2 == 0;
        let reg_index = (addr & 0xF) / 4;

        let r = if is_inside_reg {
            self.read(timer_index, reg_index)
        } else {
            0
        };
        Ok(r)
    }

    fn write_u16(&mut self, addr: u32, data: u16) -> Result<()> {
        let timer_index = (addr >> 4) & 0x3;
        let is_inside_reg = ((addr & 0xF) / 2) % 2 == 0;
        let reg_index = (addr & 0xF) / 4;

        if is_inside_reg {
            log::info!(
                "written timer register addr=0x{:X}, data=0x{:X}",
                addr,
                data
            );
            self.write(timer_index, reg_index, data);
        } else {
            log::info!(
                "written timer to garbage addr=0x{:X}, data=0x{:X}",
                addr,
                data
            );
        }
        Ok(())
    }
}
