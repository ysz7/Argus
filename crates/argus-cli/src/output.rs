//! Machine-readable output on stdout.

use std::io::Write;

/// Writes `value` to stdout as pretty-printed JSON followed by a newline.
pub(crate) fn print_json(value: &serde_json::Value) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, value)?;
    writeln!(stdout)?;
    Ok(())
}

/// Writes `value` to stdout as one line of compact JSON (JSON Lines).
pub(crate) fn print_json_line(value: &serde_json::Value) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    writeln!(stdout)?;
    stdout.flush()?;
    Ok(())
}

/// Writes human-readable text to stdout.
pub(crate) fn print_text(text: &str) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(text.as_bytes())?;
    stdout.flush()?;
    Ok(())
}
