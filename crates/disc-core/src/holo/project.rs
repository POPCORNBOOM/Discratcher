//! A design is a folder: its data in `design.json`, and every model and picture it uses inside
//! the folder too, so the folder can be moved or handed on whole.
//!
//! In `design.json` paths are relative to the folder. In memory they are absolute: opening
//! resolves them, saving copies anything from outside into `assets/` and writes them relative.

use super::model3d::{Scene, BUILTIN_SAMPLE_PATH, SCENE_VERSION};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

pub const DESIGN_FILE: &str = "design.json";
pub const ASSETS_DIR: &str = "assets";

/// Save `scene` into the design folder `dir`, making it if need be. Returns the scene as it now
/// stands, its paths pointing at the copies inside the folder.
pub fn save(dir: &Path, scene: &Scene) -> Result<Scene, String> {
    if scene.version != SCENE_VERSION {
        return Err(format!("不支持的场景版本：{}", scene.version));
    }
    std::fs::create_dir_all(dir.join(ASSETS_DIR)).map_err(|e| format!("无法创建设计文件夹：{e}"))?;
    let dir = dir.canonicalize().map_err(|e| e.to_string())?;
    let mut adopted: HashMap<String, PathBuf> = HashMap::new();
    let mut adopt = |path: &str| -> Result<String, String> {
        if path == BUILTIN_SAMPLE_PATH {
            return Ok(path.to_string());
        }
        if let Some(p) = adopted.get(path) {
            return Ok(p.to_string_lossy().into());
        }
        let p = bring_in(&dir, Path::new(path))?;
        adopted.insert(path.to_string(), p.clone());
        Ok(p.to_string_lossy().into())
    };
    let mut kept = scene.clone();
    for o in &mut kept.objects {
        o.path = adopt(&o.path)?;
    }
    for i in &mut kept.images {
        i.layer.path = adopt(&i.layer.path)?;
    }
    let mut on_disk = kept.clone();
    for o in &mut on_disk.objects {
        o.path = relative(&dir, &o.path);
    }
    for i in &mut on_disk.images {
        i.layer.path = relative(&dir, &i.layer.path);
    }
    let text = serde_json::to_vec_pretty(&on_disk).map_err(|e| e.to_string())?;
    let file = dir.join(DESIGN_FILE);
    let tmp = dir.join(format!("{DESIGN_FILE}.tmp"));
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
    Ok(kept)
}

/// Open a design: its folder, its `design.json`, or a scene file from before designs were
/// folders. Returns the scene as it was written, of whatever version, with its paths made
/// absolute (the editor brings older versions up to date), and the folder when it is a design.
pub fn open(path: &Path) -> Result<(serde_json::Value, Option<PathBuf>), String> {
    let file = if path.is_dir() { path.join(DESIGN_FILE) } else { path.to_path_buf() };
    let bytes = std::fs::read(&file).map_err(|e| format!("无法读取设计文件：{e}"))?;
    let mut scene: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| format!("不是有效的设计文件：{e}"))?;
    match scene.get("version").and_then(|v| v.as_u64()) {
        Some(v) if (1..=SCENE_VERSION as u64).contains(&v) => {}
        v => return Err(format!("不支持的场景版本：{}", v.map_or("?".into(), |v| v.to_string()))),
    }
    let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    for list in ["objects", "images"] {
        for item in scene.get_mut(list).and_then(|v| v.as_array_mut()).into_iter().flatten() {
            if let Some(p) = item.get("path").and_then(|p| p.as_str()) {
                if p != BUILTIN_SAMPLE_PATH && !Path::new(p).is_absolute() {
                    item["path"] = dir.join(p).to_string_lossy().into_owned().into();
                }
            }
        }
    }
    let is_design = file.file_name().is_some_and(|n| n == DESIGN_FILE);
    Ok((scene, is_design.then_some(dir)))
}

/// `path` as the design writes it: relative to `dir` with forward slashes when inside it.
fn relative(dir: &Path, path: &str) -> String {
    match Path::new(path).strip_prefix(dir) {
        Ok(rel) => rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"),
        Err(_) => path.to_string(),
    }
}

/// A file the design uses, brought into its folder: left where it is if already inside, else
/// copied into `assets/`. A glTF comes with the buffers and textures it names, in a folder of
/// its own so their relative paths still hold.
fn bring_in(dir: &Path, path: &Path) -> Result<PathBuf, String> {
    let src = path.canonicalize().map_err(|_| format!("找不到文件：{}", path.display()))?;
    if src.starts_with(dir) {
        return Ok(src);
    }
    let assets = dir.join(ASSETS_DIR);
    let name = src.file_name().ok_or("文件名无效")?.to_string_lossy().to_string();
    let is_gltf = src.extension().is_some_and(|e| e.eq_ignore_ascii_case("gltf"));
    if !is_gltf {
        let dest = free_name(&assets, &name, |d| same_file(&src, d))?;
        if !dest.exists() {
            std::fs::copy(&src, &dest).map_err(|e| format!("无法复制 {}：{e}", src.display()))?;
        }
        return Ok(dest);
    }
    let stem = src.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let parts = gltf_parts(&src)?;
    let folder = free_name(&assets, &stem, |d| same_file(&src, &d.join(&name)) && parts.iter().all(|p| same_file(&src.parent().unwrap().join(p), &d.join(p))))?;
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let from = src.parent().ok_or("文件名无效")?;
    for p in std::iter::once(PathBuf::from(&name)).chain(parts) {
        let to = folder.join(&p);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(from.join(&p), &to).map_err(|e| format!("无法复制 {}：{e}", from.join(&p).display()))?;
    }
    Ok(folder.join(name))
}

/// The relative files a glTF names: buffers and images that are not embedded.
fn gltf_parts(gltf: &Path) -> Result<Vec<PathBuf>, String> {
    let text = std::fs::read(gltf).map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_slice(&text).map_err(|e| format!("不是有效的 glTF：{e}"))?;
    let mut out = vec![];
    for key in ["buffers", "images"] {
        for item in json.get(key).and_then(|v| v.as_array()).into_iter().flatten() {
            let Some(uri) = item.get("uri").and_then(|u| u.as_str()) else { continue };
            if uri.starts_with("data:") || uri.contains("://") {
                continue;
            }
            let rel = PathBuf::from(percent_decode(uri));
            // Only what lies beside or below the glTF; anything else would land outside the folder.
            if rel.components().all(|c| matches!(c, Component::Normal(_))) {
                out.push(rel);
            }
        }
    }
    Ok(out)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into()
}

/// `name` in `dir`, or `name-2`, `name-3`… until one is free or `same` says it already holds
/// this file.
fn free_name(dir: &Path, name: &str, same: impl Fn(&Path) -> bool) -> Result<PathBuf, String> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 1..10_000 {
        let candidate = dir.join(if n == 1 { name.to_string() } else { format!("{stem}-{n}{ext}") });
        if !candidate.exists() || same(&candidate) {
            return Ok(candidate);
        }
    }
    Err("设计文件夹里同名文件太多".into())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artwork::Layer;
    use crate::holo::model3d::{Position, SceneImage, SceneObject};

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("design-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn scene(model: &str, picture: &str) -> Scene {
        let pos = |id: &str| Position { id: id.into(), name: String::new(), phi_deg: 0.0, crop_deg: 30.0, neutral: None, order: vec![] };
        Scene {
            version: SCENE_VERSION,
            positions: vec![pos("a"), pos("b")],
            objects: vec![
                SceneObject {
                    id: "o".into(), path: model.into(), name: "m".into(), visible: true, position_mm: [0.0, 40.0, 0.0], rotation_deg: [0.0; 3],
                    scale_mm: [20.0; 3], position_id: "a".into(), style: Default::default(), motion: Default::default(),
                    mode: Default::default(),
                    parallax: 2.0,
                    tilt_deg: 0.0,
                },
            ],
            images: vec![SceneImage { id: "i".into(), name: "p".into(), width: 4, height: 4, position_id: "b".into(), layer: Layer { path: picture.into(), ..Layer::plain() } }],
            lamp_mm: None,
        }
        .with_builtin()
    }

    impl Scene {
        /// One more model, the built-in star, which is never copied.
        fn with_builtin(mut self) -> Self {
            self.objects.push(SceneObject { id: "s".into(), path: BUILTIN_SAMPLE_PATH.into(), ..self.objects[0].clone() });
            self
        }
    }

    #[test]
    fn a_design_takes_its_files_in_and_reads_back_from_anywhere() {
        let outside = dir("outside");
        std::fs::write(outside.join("m.obj"), "v 0 0 0\n").unwrap();
        std::fs::write(outside.join("p.png"), [1u8, 2, 3]).unwrap();
        // A glTF with its buffer beside it and a texture in a folder below.
        std::fs::create_dir_all(outside.join("tex")).unwrap();
        std::fs::write(outside.join("g.bin"), [9u8; 8]).unwrap();
        std::fs::write(outside.join("tex/t%20x.png"), [7u8]).unwrap();
        std::fs::write(outside.join("tex/t x.png"), [7u8]).unwrap();
        std::fs::write(outside.join("g.gltf"), r#"{"buffers":[{"uri":"g.bin"}],"images":[{"uri":"tex/t%20x.png"},{"uri":"data:image/png;base64,AA=="}]}"#).unwrap();
        let mut s = scene(outside.join("m.obj").to_str().unwrap(), outside.join("p.png").to_str().unwrap());
        s.objects.push(SceneObject { id: "g".into(), path: outside.join("g.gltf").to_string_lossy().into(), ..s.objects[0].clone() });

        let design = dir("design");
        let saved = save(&design, &s).unwrap();
        let design = design.canonicalize().unwrap();
        assert!(saved.objects[0].path.starts_with(design.to_str().unwrap()));
        assert_eq!(saved.objects[1].path, BUILTIN_SAMPLE_PATH);
        let written: serde_json::Value = serde_json::from_slice(&std::fs::read(design.join(DESIGN_FILE)).unwrap()).unwrap();
        assert_eq!(written["objects"][0]["path"], "assets/m.obj");
        assert_eq!(written["objects"][2]["path"], "assets/g/g.gltf");
        assert_eq!(written["images"][0]["path"], "assets/p.png");
        assert!(design.join("assets/g/g.bin").exists() && design.join("assets/g/tex/t x.png").exists());

        // Saving again copies nothing more; a different file of the same name gets its own.
        save(&design, &saved).unwrap();
        std::fs::write(outside.join("m.obj"), "v 1 1 1\n").unwrap();
        let again = save(&design, &s).unwrap();
        assert!(again.objects[0].path.ends_with("assets/m-2.obj"), "{}", again.objects[0].path);
        assert_eq!(std::fs::read_dir(design.join(ASSETS_DIR)).unwrap().count(), 4);

        // Moved elsewhere, it still opens whole.
        let moved = dir("moved").join("d");
        std::fs::rename(&design, &moved).unwrap();
        let (opened, folder) = open(&moved).unwrap();
        assert_eq!(folder.as_deref(), Some(moved.as_path()));
        assert_eq!(std::fs::read(opened["images"][0]["path"].as_str().unwrap()).unwrap(), [1u8, 2, 3]);
        assert_eq!(opened["objects"][1]["path"], BUILTIN_SAMPLE_PATH);
        for d in [outside, moved.parent().unwrap().to_path_buf()] {
            std::fs::remove_dir_all(d).unwrap();
        }
    }
}
