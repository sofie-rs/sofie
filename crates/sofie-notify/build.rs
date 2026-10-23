use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;

const SOURCE: &str = "src/mails";

fn main() -> ExitCode {
    println!("cargo::rerun-if-changed={SOURCE}");
    match compile_mails() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("compiling sofie-notify's mails: {error}");
            ExitCode::FAILURE
        }
    }
}

fn compile_mails() -> Result<(), Box<dyn Error>> {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo set no OUT_DIR")?);
    let directory = out.join("mails/sofie_notify");
    std::fs::create_dir_all(&directory)?;
    for file in std::fs::read_dir(SOURCE)? {
        let path = file?.path();
        if path.extension().is_none_or(|extension| extension != "html") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("a mail's name is not Unicode")?;
        let key = format!("sofie_notify/{name}");
        let compiled = sofie_engine::compile_template(&key, &std::fs::read_to_string(&path)?)?;
        std::fs::write(directory.join(format!("{name}.sft")), compiled)?;
    }
    Ok(())
}
