use std::env::args;
use std::process::exit;

use psx_core::Psx;

fn main() {
    let args: Vec<_> = args().collect();

    if args.len() < 3 {
        println!("USAGE: {} <bios> <instructions to execute>", args[0]);
        exit(1);
    }

    let count = args[2].parse::<u32>().unwrap();

    let mut psx = Psx::new(&args[1]).unwrap();

    for _ in 0..count {
        psx.clock();
    }
}
