use clap_complete::{generate_to, shells};

use crate::cli::completion_cmd::CompletionArgs;
use crate::cli::Root;
use crate::error::Error;

pub fn generate(args: &CompletionArgs) -> Result<(), Error> {
    let mut cmd = <Root as clap::CommandFactory>::command();
    let name = "glabrs";
    let out_dir = std::env::var_os("GLABRS_COMPLETION_OUT").unwrap_or_else(|| ".".into());
    let out_path = std::path::Path::new(&out_dir);
    let file = match args {
        CompletionArgs::Bash => generate_to(shells::Bash, &mut cmd, name, out_path),
        CompletionArgs::Zsh => generate_to(shells::Zsh, &mut cmd, name, out_path),
        CompletionArgs::Fish => generate_to(shells::Fish, &mut cmd, name, out_path),
        CompletionArgs::PowerShell => generate_to(shells::PowerShell, &mut cmd, name, out_path),
    }
    .map_err(|e| Error::runtime(format!("completion: {e}")))?;
    println!("{}", file.display());
    Ok(())
}
