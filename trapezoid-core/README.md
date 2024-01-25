# trapezoid-core

[![Crates.io trapezoid-core](https://img.shields.io/crates/v/trapezoid-core)](https://crates.io/crates/trapezoid-core)
[![docs.rs (with version)](https://img.shields.io/docsrs/trapezoid-core/latest)](https://docs.rs/trapezoid-core/latest/trapezoid-core/)

This is the core of a PSX emulator [`trapezoid`](https://github.com/Amjad50/trapezoid).
It contains all the components of a working emulator, the rest is a frontend.

You can create your own frontend for this project, or use it as a server.

## Components implemented
- CPU: Mips R3000A
- GPU: backed by [`vulkano`]. `i.e. for now, you need a project running vulkano to use this`.
- SPU: produce PCM frames that should be taken out regularly by the frontend.
- CDROM: can read the contents of a PSX CDROM, and can be used to load games
    - Support XA-ADPCM audio.
- MDEC: Able to decode MDEC frames and play videos
- GTE: Geometry Transformation Engine
- DMA: Direct Memory Access
- Timers
- Interrupts
- Memory: Hosts the whole memory as a `Box<[u8]>` and provides access to it.
- Memory card: will save/load memcard to/from disk, it will save to the current folder.
    - TODO: add API to control this
- Debugging: We have an API to easily create a debugger for this emulator. This is used by the frontend [`trapezoid`].

## TODO
- Multiple tracks in cdrom
- A better API, currently the API only expose what the frontend needs. and thus doesn't have access
  to GPU, SPU, etc...
- Better docs for the API
- Add support for more CDROM formats
- Better control over audio channels


[`vulkano`]: https://github.com/vulkano-rs/vulkano
[`trapezoid`]: https://crates.io/crates/trapezoid