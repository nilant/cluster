use anyhow::Result;
use std::{fs, io::Write, path::PathBuf};

/// Raw stdout of the runs: appended to `<exe>.sol`, or, when split, one `<exe>/<instance>.sol` file per run.
pub struct Sol {
    path: PathBuf,
    split: bool,
}

impl Sol {
    /// `path` is the `<exe>` path without extension. Unless `append`, the single file is truncated;
    /// the split directory is only created (a file is replaced when its instance runs again).
    pub fn open(path: PathBuf, split: bool, append: bool) -> Result<Self> {
        let s = Self { path, split };
        match s.path.parent() {
            _ if split => fs::create_dir_all(&s.path)?,
            Some(dir) => fs::create_dir_all(dir)?,
            None => {}
        }
        if !split && !append {
            fs::write(s.file(""), "")?;
        }
        Ok(s)
    }

    /// Where the output goes: the single file, or the directory when split.
    pub fn location(&self) -> PathBuf {
        if self.split { self.path.clone() } else { self.file("") }
    }

    fn file(&self, instance: &str) -> PathBuf {
        if self.split { self.path.join(format!("{instance}.sol")) } else { self.path.with_added_extension("sol") }
    }

    pub fn add(&self, instance: &str, stdout: &[u8]) -> Result<()> {
        let file = self.file(instance);
        let opts = fs::OpenOptions::new().create(true).write(true).append(!self.split).truncate(self.split).clone();
        Ok(opts.open(file)?.write_all(stdout)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sol_single_and_split() {
        let d = std::env::temp_dir().join(format!("cl-{}-sol", std::process::id()));
        let single = Sol::open(d.join("algo"), false, false).unwrap();
        single.add("a", b"1\n").unwrap();
        single.add("b", b"2\n").unwrap();
        assert_eq!(fs::read_to_string(d.join("algo.sol")).unwrap(), "1\n2\n");
        Sol::open(d.join("algo"), false, true).unwrap().add("c", b"3\n").unwrap();
        assert_eq!(fs::read_to_string(d.join("algo.sol")).unwrap(), "1\n2\n3\n");
        assert_eq!(
            fs::read_to_string(Sol::open(d.join("algo"), false, false).map(|_| d.join("algo.sol")).unwrap()).unwrap(),
            ""
        );
        let split = Sol::open(d.join("algo"), true, false).unwrap();
        split.add("a", b"old").unwrap();
        split.add("a", b"new").unwrap();
        assert_eq!(fs::read_to_string(d.join("algo/a.sol")).unwrap(), "new");
        let _ = fs::remove_dir_all(d);
    }
}
