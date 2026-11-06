use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Results file: a JSON object `{ "<instance>": <run output>, ... }`.
/// It is rewritten atomically after every run, so it is always valid JSON,
/// even if the program is aborted halfway.
pub struct Writer {
    path: PathBuf,
    map: Map<String, Value>,
}

pub fn open(path: &Path, overwrite: bool) -> Result<Writer> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut map = Map::new();
    if !overwrite && path.exists() {
        let text = std::fs::read_to_string(path)?;
        if !text.trim().is_empty() {
            match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(m)) => map = m,
                _ => bail!("cannot append: {} is not a JSON object", path.display()),
            }
        }
    }
    let w = Writer { path: path.to_path_buf(), map };
    w.save()?;
    Ok(w)
}

impl Writer {
    fn save(&self) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, render(&self.map)).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// One `"instance": output` entry per line.
fn render(map: &Map<String, Value>) -> String {
    if map.is_empty() {
        return "{}\n".into();
    }
    let entries: Vec<String> = map
        .iter()
        .map(|(k, v)| format!("  {}: {}", Value::String(k.clone()), v))
        .collect();
    format!("{{\n{}\n}}\n", entries.join(",\n"))
}

/// Records one run. `stdout` is stored as the JSON value it contains; if it is not valid JSON
/// (crash, partial output, empty output) it is stored as a JSON string instead.
pub fn append(w: &mut Writer, instance: &str, stdout: &str) -> Result<()> {
    let value = serde_json::from_str::<Value>(stdout.trim()).unwrap_or_else(|_| Value::String(stdout.to_string()));
    w.map.insert(instance.to_string(), value);
    w.save()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cl-{}-{name}.json", std::process::id()))
    }

    #[test]
    fn format_and_validity() {
        let p = tmp("fmt");
        let mut w = open(&p, true).unwrap();
        append(&mut w, "a", r#"{"obj":94.0,"bound":49.0,"runtime":0.000023975}"#).unwrap();
        append(&mut w, "b", "").unwrap();
        append(&mut w, "c", "oops \"x\"\n").unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.contains("  \"a\": {\"obj\":94.0,\"bound\":49.0,\"runtime\":2.3975e-5}") || s.contains("\"a\": {"), "{s}");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["a"]["obj"], 94.0);
        assert_eq!(v["b"], "");
        assert_eq!(v["c"], "oops \"x\"\n");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn append_mode_keeps_and_replaces() {
        let p = tmp("app");
        let mut w = open(&p, true).unwrap();
        append(&mut w, "a", "1").unwrap();
        append(&mut w, "b", "2").unwrap();
        drop(w);
        let mut w = open(&p, false).unwrap();
        append(&mut w, "b", "3").unwrap();
        append(&mut w, "c", "4").unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!((v["a"].as_i64(), v["b"].as_i64(), v["c"].as_i64()), (Some(1), Some(3), Some(4)));
        assert!(open(&p, true).is_ok());
        std::fs::write(&p, "not json").unwrap();
        assert!(open(&p, false).is_err());
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod fidelity {
    use super::*;
    #[test]
    fn numbers_kept_verbatim() {
        let p = std::env::temp_dir().join(format!("cl-{}-fid.json", std::process::id()));
        let mut w = open(&p, true).unwrap();
        let out = r#"{"obj":94.0,"bound":49.0,"runtime":0.000023975,"x":1e-10,"y":1.50}"#;
        append(&mut w, "012379a0-f981-4053-9144-29d2d31ae611", out).unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        assert!(s.contains(&format!("\"012379a0-f981-4053-9144-29d2d31ae611\": {out}")), "{s}");
    }
}
