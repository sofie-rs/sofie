use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;

const MAILS: &str = "src/mails";

fn main() -> ExitCode {
    println!("cargo::rerun-if-changed={MAILS}");
    match compile_mails() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("compiling sofie-tenancy's mails: {error}");
            ExitCode::FAILURE
        }
    }
}

fn compile_mails() -> Result<(), Box<dyn Error>> {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo set no OUT_DIR")?);
    let directory = out.join("mails/sofie_tenancy");
    std::fs::create_dir_all(&directory)?;
    for file in std::fs::read_dir(MAILS)? {
        let path = file?.path();
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if path
            .extension()
            .is_some_and(|extension| extension == "html")
        {
            let key = format!("sofie_tenancy/{name}");
            let compiled = sofie_engine::compile_template(&key, &std::fs::read_to_string(&path)?)?;
            std::fs::write(directory.join(format!("{name}.sft")), compiled)?;
        }
    }
    Ok(())
}
