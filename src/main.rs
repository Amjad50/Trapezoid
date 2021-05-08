use std::env::args;
use std::process::exit;

use psx_core::Psx;

fn main() {
    env_logger::builder().format_timestamp(None).init();
    let args: Vec<_> = args().collect();

    if args.len() < 2 {
        println!("USAGE: {} <bios>", args[0]);
        exit(1);
    }

    let mut psx = Psx::new(&args[1]).unwrap();
    loop {
        psx.clock();
    }
}
