use anyhow::Result;
use std::fs::{File, OpenOptions};
use std::path::Path;

/// CSV writer shared behind a Mutex so rows never interleave.
pub type Writer = csv::Writer<File>;

pub fn open(path: &Path, overwrite: bool) -> Result<Writer> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let exists = path.exists() && std::fs::metadata(path)?.len() > 0 && !overwrite;
    let file = if overwrite {
        File::create(path)?
    } else {
        OpenOptions::new().create(true).append(true).open(path)?
    };
    let mut w = csv::WriterBuilder::new().has_headers(false).from_writer(file);
    if !exists {
        w.write_record(["instance", "stdout"])?;
        w.flush()?;
    }
    Ok(w)
}

pub fn append(w: &mut Writer, instance: &str, stdout: &str) -> Result<()> {
    w.write_record([instance, stdout])?;
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        let p = std::env::temp_dir().join(format!("cl-test-{}.csv", std::process::id()));
        let mut w = open(&p, true).unwrap();
        append(&mut w, "a.txt", "1,2\n\"x\"").unwrap();
        drop(w);
        let s = std::fs::read_to_string(&p).unwrap();
        assert_eq!(s, "instance,stdout\na.txt,\"1,2\n\"\"x\"\"\"\n");
        let _ = std::fs::remove_file(p);
    }
}
