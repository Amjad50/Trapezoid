use std::env::args;
use std::process::exit;

use psx_core::Psx;

fn main() {
    let args: Vec<_> = args().collect();

    if args.len() < 2 {
        println!("USAGE: {} <bios>", args[0]);
        exit(1);
    }

    let mut psx = Psx::new(&args[1]).unwrap();

    for _ in 0..20 {
        psx.clock();
    }
}
