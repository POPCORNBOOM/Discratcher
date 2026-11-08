//! Composition files: the layers of a carve with their pictures inside, so one file can be
//! moved or shared. The layer fields belong to the front end; this only swaps each layer's
//! `path` for the picture's bytes and back.

use base64::Engine;
use serde_json::{json, Map, Value};
use std::path::Path;

const FORMAT: &str = "discratcher-composition";
const VERSION: u64 = 1;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

pub fn pack(layers: &[Map<String, Value>]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(layers.len());
    for l in layers {
        let path = l.get("path").and_then(Value::as_str).ok_or("图层缺少图像")?;
        let bytes = std::fs::read(path).map_err(|e| format!("无法读取图像 {path}：{e}"))?;
        let mut l = l.clone();
        l.remove("path");
        let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase();
        l.insert("imageType".into(), ext.into());
        l.insert("image".into(), B64.encode(bytes).into());
        out.push(Value::Object(l));
    }
    serde_json::to_vec(&json!({ "format": FORMAT, "version": VERSION, "layers": out })).map_err(|e| e.to_string())
}

/// Writes each picture into `images` once, named by its content, and returns the layers
/// pointing at those copies.
pub fn unpack(bytes: &[u8], images: &Path) -> Result<Vec<Map<String, Value>>, String> {
    let bad = || "不是 Discratcher 构图文件".to_string();
    let doc: Value = serde_json::from_slice(bytes).map_err(|_| bad())?;
    if doc["format"] != FORMAT {
        return Err(bad());
    }
    if doc["version"].as_u64().unwrap_or(0) > VERSION {
        return Err("这个构图文件来自更新的版本，请先更新应用".into());
    }
    std::fs::create_dir_all(images).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for l in doc["layers"].as_array().ok_or_else(bad)? {
        let mut l = l.as_object().ok_or_else(bad)?.clone();
        let data = l.remove("image").and_then(|v| v.as_str().map(str::to_owned)).ok_or_else(bad)?;
        let data = B64.decode(data).map_err(|_| bad())?;
        let ext: String = l.remove("imageType").and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_else(|| "png".into());
        let ext: String = ext.chars().filter(char::is_ascii_alphanumeric).take(5).collect();
        let path = images.join(format!("{:016x}.{ext}", fnv(&data)));
        if !path.exists() {
            std::fs::write(&path, &data).map_err(|e| e.to_string())?;
        }
        l.insert("path".into(), path.to_string_lossy().into_owned().into());
        out.push(l);
    }
    Ok(out)
}

fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_layers_and_pictures() {
        let dir = std::env::temp_dir().join(format!("disc-core-comp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.PNG");
        std::fs::write(&src, b"picture bytes").unwrap();
        let layer = json!({ "id": "l1", "path": src.to_string_lossy(), "x": 0.1, "size": 0.5, "name": "a.PNG" });
        let packed = pack(&[layer.as_object().unwrap().clone()]).unwrap();
        let back = unpack(&packed, &dir.join("images")).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0]["x"], 0.1);
        assert_eq!(back[0]["name"], "a.PNG");
        let path = back[0]["path"].as_str().unwrap();
        assert!(path.ends_with(".png"));
        assert_eq!(std::fs::read(path).unwrap(), b"picture bytes");
        assert!(unpack(b"{}", &dir).is_err());
    }
}
