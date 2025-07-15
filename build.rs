use std::{env, io::Error};
use clap::CommandFactory;
use clap_complete::{aot::Bash, generate_to};

include!("src/cli.rs");

fn main() -> Result<(), Error> {
    let out_dir = match env::var_os("OUT_DIR") {
        None => return Ok(()),
        Some(outdir) => outdir,
    };

    let path = generate_to(
        Bash,
        &mut <Cli as CommandFactory>::command(),
        "smithy",
        out_dir
    )?;

    println!("cargo:warning=completion file is generated: {path:?}");

    Ok(())
}
