//! `tracing` setup: JSON (default) or human-readable logs to stdout or a file, plus a panic hook
//! that routes panics into the same log. Log lines carry ids, statuses and timings only.

use std::fs::{File, OpenOptions};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::Layer;
use tracing_subscriber::fmt::{self, writer::BoxMakeWriter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::{LogFormat, LoggingConfig};

/// Install the global subscriber. Fails if the filter or the log file is invalid.
pub fn init(config: &LoggingConfig) -> Result<(), String> {
    let filter = config.filter()?;
    let (writer, ansi) = match &config.file {
        Some(path) => {
            let file =
                RotatingFile::open(path, config.max_file_mb * 1024 * 1024, config.max_files)?;
            (BoxMakeWriter::new(Mutex::new(file)), false)
        }
        None => (
            BoxMakeWriter::new(std::io::stdout),
            std::io::stdout().is_terminal(),
        ),
    };
    let layer = match config.format {
        LogFormat::Json => fmt::layer()
            .json()
            .with_current_span(true)
            .with_span_list(false)
            .with_writer(writer)
            .with_filter(filter)
            .boxed(),
        LogFormat::Pretty => fmt::layer()
            .with_ansi(ansi)
            .with_writer(writer)
            .with_filter(filter)
            .boxed(),
    };
    tracing_subscriber::registry()
        .with(layer)
        .try_init()
        .map_err(|e| format!("cannot initialise logging: {e}"))
}

/// An append-only log file that rotates by size: when a write would take it past `max_bytes`,
/// `<file>.<n-1>` becomes `<file>.<n>` (the oldest beyond `keep` is deleted), `<file>` becomes
/// `<file>.1`, and a new `<file>` is started. Each write is one whole log line, so lines are
/// never split across files.
///
/// On Windows a rename fails while another process holds the file without delete sharing (e.g.
/// a tailing editor). The writer then keeps appending to `<file>` and tries again after another
/// `RETRY_AFTER_BYTES`, so no log line is lost to rotation.
pub struct RotatingFile {
    path: PathBuf,
    /// `None` only while rotating.
    file: Option<File>,
    size: u64,
    /// 0: never rotate.
    max_bytes: u64,
    keep: usize,
    /// After a failed rotation, the size at which to try again.
    retry_at: u64,
}

/// Growth after a failed rotation before the next attempt.
const RETRY_AFTER_BYTES: u64 = 1024 * 1024;

impl RotatingFile {
    pub fn open(path: &Path, max_bytes: u64, keep: usize) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create log directory `{}`: {e}", dir.display()))?;
        }
        let file = Self::append(path)
            .map_err(|e| format!("cannot open log file `{}`: {e}", path.display()))?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(RotatingFile {
            path: path.to_path_buf(),
            file: Some(file),
            size,
            max_bytes,
            keep: keep.max(1),
            retry_at: 0,
        })
    }

    fn append(path: &Path) -> std::io::Result<File> {
        OpenOptions::new().create(true).append(true).open(path)
    }

    fn numbered(&self, n: usize) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    fn shift(&self) -> std::io::Result<()> {
        let oldest = self.numbered(self.keep);
        if oldest.exists() {
            std::fs::remove_file(&oldest)?;
        }
        for n in (1..self.keep).rev() {
            let from = self.numbered(n);
            if from.exists() {
                std::fs::rename(&from, self.numbered(n + 1))?;
            }
        }
        std::fs::rename(&self.path, self.numbered(1))
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        // Close our handle first: Windows cannot rename an open file.
        self.file = None;
        let shifted = self.shift();
        // Whatever happened, append to `<file>` again (a new, empty one after a rotation).
        let file = Self::append(&self.path)?;
        self.size = file.metadata().map(|m| m.len()).unwrap_or(0);
        self.file = Some(file);
        shifted
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let incoming = buf.len() as u64;
        if self.max_bytes > 0
            && self.size > 0
            && self.size + incoming > self.max_bytes
            && self.size >= self.retry_at
        {
            match self.rotate() {
                Ok(()) => self.retry_at = 0,
                Err(e) => {
                    self.retry_at = self.size + RETRY_AFTER_BYTES;
                    let _ = writeln!(std::io::stderr(), "veriface: log rotation failed: {e}");
                }
            }
        }
        let file = match &mut self.file {
            Some(file) => file,
            None => self.file.insert(Self::append(&self.path)?),
        };
        let written = file.write(buf)?;
        self.size += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match &mut self.file {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

/// Log panics (message and location) through `tracing` instead of stderr.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_owned());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        tracing::error!(panic = %message, location = location.as_deref(), "panic");
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_by_size_and_keeps_the_newest_files() {
        let dir = std::env::temp_dir().join(format!("veriface-rotate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("logs").join("x.log");
        let mut log = RotatingFile::open(&path, 100, 2).unwrap();
        let line = |c: char| {
            format!(
                "{}
",
                c.to_string().repeat(39)
            )
        }; // 40 bytes
        for c in ['a', 'b', 'c', 'd', 'e', 'f', 'g'] {
            log.write_all(line(c).as_bytes()).unwrap();
        }
        log.flush().unwrap();
        drop(log);
        let read = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
        let numbered = |n: u8| {
            let mut name = path.as_os_str().to_owned();
            name.push(format!(".{n}"));
            PathBuf::from(name)
        };
        // 2 lines (80 bytes) per file; 7 lines: g | e f | c d, and a b dropped (keep = 2).
        assert_eq!(read(&path), line('g'));
        assert_eq!(read(&numbered(1)), format!("{}{}", line('e'), line('f')));
        assert_eq!(read(&numbered(2)), format!("{}{}", line('c'), line('d')));
        assert!(!numbered(3).exists());
        assert!(!numbered(0).exists());

        // Reopening continues the current file; 0 = never rotate.
        let mut log = RotatingFile::open(&path, 0, 2).unwrap();
        for _ in 0..10 {
            log.write_all(line('h').as_bytes()).unwrap();
        }
        drop(log);
        assert_eq!(read(&path).len(), 11 * 40);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
