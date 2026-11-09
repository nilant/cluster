use anyhow::Result;
use std::{fs, fs::File, io::Write, path::Path};

/// Results file: one `# <instance>` header per run, followed by the run's standard output.
/// Append-only, so it is complete up to the last finished run even if the program is aborted.
pub struct Writer(File);

impl Writer {
    /// Creates (or truncates) the file; with `append`, adds to the end of an existing one.
    pub fn open(path: &Path, append: bool) -> Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = fs::OpenOptions::new().create(true).write(true).append(append).truncate(!append).open(path)?;
        Ok(Self(file))
    }

    /// Records one run. `stdout` is written as is, with a newline after it (nothing if it is empty).
    pub fn add(&mut self, instance: &str, stdout: &str) -> Result<()> {
        let sep = if stdout.is_empty() { "" } else { "\n" };
        // one write per block, so blocks never interleave
        self.0.write_all(format!("# {instance}\n{stdout}{sep}").as_bytes())?;
        Ok(self.0.flush()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("cl-{}-{name}.txt", std::process::id()))
    }

    #[test]
    fn block_format() {
        let p = tmp("fmt");
        let mut w = Writer::open(&p, false).unwrap();
        w.add("inst_1", r#"{"obj":94.0,"runtime":0.000023975}"#).unwrap();
        w.add("inst_2", "iteration 1: 120\n  indented\ndone").unwrap();
        w.add("inst_3", "").unwrap();
        w.add("inst_4", "last").unwrap();
        let want = "# inst_1\n{\"obj\":94.0,\"runtime\":0.000023975}\n# inst_2\niteration 1: 120\n  indented\ndone\n# inst_3\n# inst_4\nlast\n";
        assert_eq!(fs::read_to_string(&p).unwrap(), want);
        let _ = fs::remove_file(p);
    }

    #[test]
    fn append_keeps_earlier_content_and_default_truncates() {
        let p = tmp("app");
        Writer::open(&p, false).unwrap().add("a", "1").unwrap();
        Writer::open(&p, true).unwrap().add("b", "2").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "# a\n1\n# b\n2\n");
        Writer::open(&p, false).unwrap().add("c", "3").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "# c\n3\n");
        let _ = fs::remove_file(p);
    }
}
