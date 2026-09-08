//! Where a command's report goes, and how a write failure becomes an exit
//! code.
//!
//! Commands that emit a structured document share one contract: the document
//! goes wholly to standard output or wholly to `--output`, never split across
//! both, and a downstream `head` that closes the pipe is not a failure. Both
//! halves live here so no two commands can drift on either point.

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;

/// Where the report goes.
///
/// Without `--output` the human format keeps its stream split — error rows on
/// stderr, everything else on stdout — so a shell can separate them. With it,
/// every line of the report goes to the file whatever the format, and the
/// terminal stays clean.
pub(crate) enum Report {
    Console,
    File(io::BufWriter<fs::File>),
}

impl Report {
    pub(crate) fn open(path: Option<&Path>) -> io::Result<Self> {
        match path {
            Some(path) => Ok(Self::File(io::BufWriter::new(fs::File::create(path)?))),
            None => Ok(Self::Console),
        }
    }

    /// Emit one line of the human report.
    pub(crate) fn line(&mut self, line: &str, is_error: bool) -> io::Result<()> {
        match self {
            Self::Console if is_error => {
                eprintln!("{line}");
                Ok(())
            }
            Self::Console => {
                println!("{line}");
                Ok(())
            }
            Self::File(out) => writeln!(out, "{line}"),
        }
    }

    /// Emit a whole structured document through `write`.
    ///
    /// Standard output is line-buffered, which for a document of any size
    /// means one write syscall per line. Buffering it here, as the file arm
    /// already is, is the difference between thousands of syscalls and
    /// millions on a large model.
    pub(crate) fn structured(
        &mut self,
        write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    ) -> io::Result<()> {
        match self {
            Self::Console => {
                let mut out = io::BufWriter::with_capacity(64 * 1024, io::stdout().lock());
                write(&mut out)?;
                out.flush()
            }
            Self::File(out) => write(out),
        }
    }

    pub(crate) fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Console => Ok(()),
            Self::File(out) => out.flush(),
        }
    }
}

/// Fold the outcome of writing a structured document into the exit code the
/// command had already earned.
///
/// A broken pipe keeps that code: `rossi validate --format json | head` closes
/// the reader on purpose, and reporting it as a write failure would turn a
/// normal shell idiom into an error. Any other write failure overrides the
/// code, because a truncated document is worse than a reported failure.
/// `command` names the subcommand for the message.
pub(crate) fn finish_structured_output(
    command: &str,
    output_result: io::Result<()>,
    command_exit: ExitCode,
    stderr: &mut impl Write,
) -> ExitCode {
    match output_result {
        Ok(()) => command_exit,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => command_exit,
        Err(e) => {
            let _ = writeln!(stderr, "rossi {command}: failed to write output: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_failure_is_reported_and_exits_nonzero() {
        let mut stderr = Vec::new();

        let exit = finish_structured_output(
            "validate",
            Err(io::Error::other("write failed")),
            ExitCode::SUCCESS,
            &mut stderr,
        );

        assert_eq!(exit, ExitCode::from(1));
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "rossi validate: failed to write output: write failed\n"
        );
    }

    #[test]
    fn broken_pipe_preserves_the_command_exit() {
        let mut stderr = Vec::new();
        let failed = ExitCode::from(1);

        let exit = finish_structured_output(
            "validate",
            Err(io::Error::from(io::ErrorKind::BrokenPipe)),
            failed,
            &mut stderr,
        );

        assert_eq!(exit, failed);
        assert!(stderr.is_empty());
    }
}
