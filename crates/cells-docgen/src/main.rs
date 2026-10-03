//! Usage: cells-docgen <spec>            print one document (e.g. chain-1000)
//!        cells-docgen --legacy <spec>   same, with <op> rewritten as <number> math
//!        cells-docgen --sweep           print the default sweep specs, one per line
fn main() {
    let mut args = std::env::args().skip(1);
    let mut arg = args.next().unwrap_or_default();
    let legacy = arg == "--legacy";
    if legacy {
        arg = args.next().unwrap_or_default();
    }
    if arg == "--sweep" {
        for s in cells_docgen::DEFAULT_SWEEP {
            println!("{s}");
        }
        return;
    }
    match cells_docgen::from_spec(&arg) {
        Some(doc) => print!("{}", if legacy { cells_docgen::to_legacy(&doc) } else { doc }),
        None => {
            eprintln!("usage: cells-docgen <points|chain|fanout|aliases>-<N> | grid-<N>x<L> | --sweep");
            std::process::exit(2);
        }
    }
}
