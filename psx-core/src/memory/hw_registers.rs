pub static HW_REGISTERS: phf::Map<&'static str, u32> = phf::phf_map! {
    // Gpu
    "GPU_STAT" => 0x1F801814,

    // Interrupts
    "INT_STAT" => 0x1F801070,
    "INT_MASK" => 0x1F801074,

    // Dma
    "DMA_MDECIN_MADR" => 0x1F801080,
    "DMA_MDECIN_BCR" => 0x1F801084,
    "DMA_MDECIN_CHCR" => 0x1F801088,
    "DMA_MDECOUT_MADR" => 0x1F801090,
    "DMA_MDECOUT_BCR" => 0x1F801094,
    "DMA_MDECOUT_CHCR" => 0x1F801098,
    "DMA_GPU_MADR" => 0x1F8010A0,
    "DMA_GPU_BCR" => 0x1F8010A4,
    "DMA_GPU_CHCR" => 0x1F8010A8,
    "DMA_CDROM_MADR" => 0x1F8010B0,
    "DMA_CDROM_BCR" => 0x1F8010B4,
    "DMA_CDROM_CHCR" => 0x1F8010B8,
    "DMA_SPU_MADR" => 0x1F8010C0,
    "DMA_SPU_BCR" => 0x1F8010C4,
    "DMA_SPU_CHCR" => 0x1F8010C8,
    "DMA_PIO_MADR" => 0x1F8010D0,
    "DMA_PIO_BCR" => 0x1F8010D4,
    "DMA_PIO_CHCR" => 0x1F8010D8,
    "DMA_OTC_MADR" => 0x1F8010E0,
    "DMA_OTC_BCR" => 0x1F8010E4,
    "DMA_OTC_CHCR" => 0x1F8010E8,
    "DMA_CONTROL" => 0x1F8010F0,
    "DMA_INTERRUPT" => 0x1F8010F4,

    // Timers
    "TIMER0_COUNTER" => 0x1F801100,
    // "TIMER0_MODE" => 0x1F801104, // TODO: this reset after read, we can'the
                                    // make the debugger reset it
    "TIMER0_TARGET" => 0x1F801108,
    "TIMER1_COUNTER" => 0x1F801110,
    // "TIMER1_MODE" => 0x1F801114,
    "TIMER1_TARGET" => 0x1F801118,
    "TIMER2_COUNTER" => 0x1F801120,
    // "TIMER2_MODE" => 0x1F801124,
    "TIMER2_TARGET" => 0x1F801128,

    // MDEC
    // "MDEC_DATA" => 0x1F801820, // TODO: reading this pulls from the fifo
    "MDEC_STATUS" => 0x1F801824,
};
