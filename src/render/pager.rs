use std::io::{IsTerminal, Read, Write};
use std::process::{Command, Stdio};

use crate::error::Error;

/// Run a pager over `content` unless disabled or stdout is not a terminal.
/// Pager selection: `GLABRS_PAGER` > `PAGER` > `less`.
pub fn page(content: &str, no_pager: bool) -> Result<(), Error> {
    if no_pager || !std::io::stdout().is_terminal() {
        print!("{content}");
        return Ok(());
    }
    let pager = std::env::var("GLABRS_PAGER")
        .or_else(|_| std::env::var("PAGER"))
        .unwrap_or_else(|_| "less".to_string());
    let mut child = Command::new(&pager)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| Error::runtime(format!("pager {pager}: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        // The pager may exit early (e.g. `less` quit with `q`), surfacing as
        // EPIPE. That is normal interactive behavior, not an error.
        if stdin.write_all(content.as_bytes()).is_err() {
            let _ = stdin.flush();
        }
    }
    let status = child
        .wait()
        .map_err(|e| Error::runtime(format!("pager: {e}")))?;
    if !status.success() {
        return Err(Error::runtime(format!("pager {pager} exited {status}")));
    }
    Ok(())
}

/// Read text from a file, or stdin when path is `-`.
pub fn read_input(path: &str) -> Result<String, Error> {
    let mut s = String::new();
    if path == "-" {
        std::io::stdin().read_to_string(&mut s)?;
    } else {
        s = std::fs::read_to_string(path)?;
    }
    Ok(s)
}
