use anyhow::{Result, bail};
use serde_json::{Map, Value};
use std::{fs, path::PathBuf};

/// Results file: a JSON object with one `"<instance>": <run output>` entry per line.
/// It is rewritten atomically after every run, so it is valid JSON even if the program is aborted.
pub struct Writer {
    path: PathBuf,
    map: Map<String, Value>,
}

impl Writer {
    /// Creates (or truncates) the file; with `append`, keeps the entries of an existing file.
    pub fn open(path: PathBuf, append: bool) -> Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let map = match fs::read_to_string(&path) {
            Ok(text) if append && !text.trim().is_empty() => match serde_json::from_str(&text) {
                Ok(Value::Object(m)) => m,
                _ => bail!("cannot append: {} is not a JSON object", path.display()),
            },
            _ => Map::new(),
        };
        let w = Self { path, map };
        w.save()?;
        Ok(w)
    }

    /// Records one run, replacing an earlier entry for the same instance. `stdout` is stored as the
    /// JSON value it contains, or as a JSON string if it is not valid JSON (crash, partial or no output).
    pub fn add(&mut self, instance: &str, stdout: &str) -> Result<()> {
        let value = serde_json::from_str(stdout.trim()).unwrap_or_else(|_| Value::from(stdout));
        self.map.insert(instance.into(), value);
        self.save()
    }

    fn save(&self) -> Result<()> {
        let lines: Vec<_> = self.map.iter().map(|(k, v)| format!("  {}: {v}", Value::from(k.as_str()))).collect();
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, format!("{{\n{}\n}}\n", lines.join(",\n")))?;
        Ok(fs::rename(&tmp, &self.path)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cl-{}-{name}.json", std::process::id()))
    }

    #[test]
    fn format_and_fallbacks() {
        let p = tmp("fmt");
        let mut w = Writer::open(p.clone(), false).unwrap();
        let out = r#"{"obj":94.0,"bound":49.0,"runtime":0.000023975,"x":1e-10,"y":1.50}"#;
        w.add("012379a0-f981", out).unwrap();
        w.add("b", "").unwrap();
        w.add("c", "oops \"x\"\n").unwrap();
        let s = fs::read_to_string(&p).unwrap();
        assert!(s.starts_with(&format!("{{\n  \"012379a0-f981\": {out},\n")), "numbers kept verbatim: {s}");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!((v["b"].as_str(), v["c"].as_str()), (Some(""), Some("oops \"x\"\n")));
        let _ = fs::remove_file(p);
    }

    #[test]
    fn append_keeps_and_replaces() {
        let p = tmp("app");
        let mut w = Writer::open(p.clone(), false).unwrap();
        w.add("a", "1").unwrap();
        w.add("b", "2").unwrap();
        let mut w = Writer::open(p.clone(), true).unwrap();
        w.add("b", "3").unwrap();
        w.add("c", "4").unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!((v["a"].as_i64(), v["b"].as_i64(), v["c"].as_i64()), (Some(1), Some(3), Some(4)));
        assert!(Writer::open(p.clone(), false).is_ok());
        fs::write(&p, "not json").unwrap();
        assert!(Writer::open(p.clone(), true).is_err());
        let _ = fs::remove_file(p);
    }
}
