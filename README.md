<!-- <p align="center">
  <a href="https://github.com/Amjad50/trapezoid"><img alt="trapezoid" src="images/logo.svg" width="60%"></a>
  <p align="center">PSX emulator in <em>Rust</em></p>
</p> -->
# Trapezoid

[![Build status](https://github.com/Amjad50/trapezoid/workflows/Rust/badge.svg)](https://actions-badge.atrox.dev/Amjad50/trapezoid/goto)
[![dependency status](https://deps.rs/repo/github/Amjad50/trapezoid/status.svg)](https://deps.rs/repo/github/Amjad50/trapezoid)
[![license](https://img.shields.io/github/license/Amjad50/trapezoid)](./LICENSE)

[![Crates.io trapezoid](https://img.shields.io/crates/v/trapezoid)](https://crates.io/crates/trapezoid)
[![docs.rs (with version)](https://img.shields.io/docsrs/trapezoid/latest)](https://docs.rs/trapezoid/latest/trapezoid/)

**trapezoid** is a [PSX/PS1](https://en.wikipedia.org/wiki/PlayStation_(console)) emulator built from scratch using [Rust].

This is a personal project for fun and to experience emulating hardware and connecting them together.

### Building and installation

#### Installing
You can install `trapezoid` from [`crates.io`](https://crates.io/crates/trapezoid) using `cargo`:
```
cargo install trapezoid
```

#### Building
If you want to experience the latest development version, you can build `trapezoid` yourself.
```
cargo build --release
```
> The emulator will be slow without optimization, that's why we have `opt-level = 2` in `debug` profile.

### Emulator core
The emulator core is implemented as a library in [`trapezoid-core`], this library is the emulator core, and contain
all the components. You can easily take the core and build a frontend around it, or use it as a server.

Check the [`trapezoid-core`] for more info and documentation.

### Frontend


#### Controls
The Frontend implementations has its own controls mapping, this can be configured
if you decide to use [`trapezoid-core`] directly

##### Keyboard



| keyboard  | PSX controller |
| --------- | -------------- |
| Enter     | Start          |
| Backspace | Select         |
| Num1      | L1             |
| Num2      | L2             |
| Num3      | L3             |
| Num0      | R1             |
| Num9      | R2             |
| Num8      | R3             |
| W         | Up             |
| S         | Down           |
| D         | Right          |
| A         | Left           |
| I         | Triangle       |
| K         | X              |
| L         | Circle         |
| J         | Square         |

### License
This project is under [MIT](./LICENSE) license.

NES is a product and/or trademark of Nintendo Co., Ltd. Nintendo Co., Ltd. and is not affiliated in any way with Plastic or its author

### References
Most of the documentation for PSX components can be found in the [consoledev website](https://psx-spx.consoledev.net/)

[Rust]: https://www.rust-lang.org/
[`trapezoid-core`]: ./trapezoid-core/README.md