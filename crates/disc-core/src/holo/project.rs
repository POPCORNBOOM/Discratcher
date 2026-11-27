//! A design is a folder: its data in `design.json`, and every model and picture it uses inside
//! the folder too, so the folder can be moved or handed on whole.
//!
//! In `design.json` paths are relative to the folder. In memory they are absolute: opening
//! resolves them, saving copies anything from outside into `assets/` and writes them relative.

use super::model3d::{Scene, BUILTIN_SAMPLE_PATH, SCENE_VERSION};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

pub const DESIGN_FILE: &str = "design.json";
pub const ASSETS_DIR: &str = "assets";

/// What sits at a candidate path before saving a design there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FolderState {
    Missing,
    Empty,
    Design,
    Other,
}

/// Inspect `dir` so the caller can decide whether to save, confirm overwrite, or refuse.
pub fn folder_state(dir: &Path) -> Result<FolderState, String> {
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FolderState::Missing),
        Err(e) => return Err(e.to_string()),
        Ok(_) => {}
    }
    if !dir.is_dir() {
        return Ok(FolderState::Other);
    }
    if dir.join(DESIGN_FILE).is_file() {
        return Ok(FolderState::Design);
    }
    let mut entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    if entries.next().transpose().map_err(|e| e.to_string())?.is_none() {
        Ok(FolderState::Empty)
    } else {
        Ok(FolderState::Other)
    }
}

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
    for p in &mut kept.positions {
        for c in p.crops.iter_mut().flatten() {
            if let Some(path) = c.mask_path_mut() {
                *path = adopt(path)?;
            }
        }
    }
    let mut on_disk = kept.clone();
    for o in &mut on_disk.objects {
        o.path = relative(&dir, &o.path);
    }
    for i in &mut on_disk.images {
        i.layer.path = relative(&dir, &i.layer.path);
    }
    for p in &mut on_disk.positions {
        for c in p.crops.iter_mut().flatten() {
            if let Some(path) = c.mask_path_mut() {
                *path = relative(&dir, path);
            }
        }
    }
    let text = serde_json::to_vec_pretty(&on_disk).map_err(|e| e.to_string())?;
    let file = dir.join(DESIGN_FILE);
    let tmp = dir.join(format!("{DESIGN_FILE}.tmp"));
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
    prune_unused_assets(&dir, &kept);
    Ok(kept)
}

/// Open a design: its folder, its `design.json`, or a scene file from before designs were
/// folders. Returns the scene as it was written, of whatever version, with its paths made
/// absolute (the editor brings older versions up to date), and the folder when it is a design.
pub fn open(path: &Path) -> Result<(serde_json::Value, Option<PathBuf>), String> {
    let is_dir = path.is_dir();
    let file = if is_dir { path.join(DESIGN_FILE) } else { path.to_path_buf() };
    let bytes = std::fs::read(&file).map_err(|e| {
        if is_dir && e.kind() == std::io::ErrorKind::NotFound {
            "这个文件夹不是设计".into()
        } else {
            format!("无法读取设计文件：{e}")
        }
    })?;
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
    for pos in scene.get_mut("positions").and_then(|v| v.as_array_mut()).into_iter().flatten() {
        for crop in pos.get_mut("crops").and_then(|v| v.as_array_mut()).into_iter().flatten() {
            if crop.get("kind").and_then(|k| k.as_str()) == Some("mask") {
                if let Some(p) = crop.get("path").and_then(|p| p.as_str()) {
                    if !Path::new(p).is_absolute() {
                        crop["path"] = dir.join(p).to_string_lossy().into_owned().into();
                    }
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

/// Remove files under `<dir>/assets/` no longer named by `scene`, and any subfolders left empty.
/// Only touches real files and directories strictly inside `<dir>/assets`; never follows symlinks.
fn prune_unused_assets(dir: &Path, scene: &Scene) {
    let assets_path = dir.join(ASSETS_DIR);
    let Ok(meta) = std::fs::symlink_metadata(&assets_path) else { return };
    if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
        return;
    }
    let Ok(assets_root) = assets_path.canonicalize() else { return };
    if !assets_root.starts_with(dir) || assets_root == dir {
        return;
    }

    let mut kept_files = HashSet::new();
    let mut kept_dirs = HashSet::new();
    let refs = scene
        .objects
        .iter()
        .map(|o| o.path.as_str())
        .chain(scene.images.iter().map(|i| i.layer.path.as_str()))
        .chain(scene.positions.iter().filter_map(|p| p.crops.as_ref()).flatten().filter_map(|c| c.mask_path()));
    for p in refs {
        if p == BUILTIN_SAMPLE_PATH {
            continue;
        }
        let Ok(canon) = Path::new(p).canonicalize() else { continue };
        if !canon.starts_with(&assets_root) || canon == assets_root {
            continue;
        }
        kept_files.insert(canon.clone());
        if canon.extension().is_some_and(|e| e.eq_ignore_ascii_case("gltf")) {
            if let Some(parent) = canon.parent() {
                if parent != assets_root && parent.starts_with(&assets_root) {
                    kept_dirs.insert(parent.to_path_buf());
                } else if let Ok(parts) = gltf_parts(&canon) {
                    for part in parts {
                        if let Ok(part_canon) = parent.join(part).canonicalize() {
                            if part_canon.starts_with(&assets_root) {
                                kept_files.insert(part_canon);
                            }
                        }
                    }
                }
            }
        }
    }

    prune_dir(&assets_root, &assets_root, &kept_files, &kept_dirs);
}

fn prune_dir(
    assets_root: &Path,
    current: &Path,
    kept_files: &HashSet<PathBuf>,
    kept_dirs: &HashSet<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(current) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        let ft = meta.file_type();
        if ft.is_symlink() {
            continue;
        }
        let Ok(canon) = path.canonicalize() else { continue };
        if !canon.starts_with(assets_root) || canon == assets_root {
            continue;
        }
        if ft.is_dir() {
            if kept_dirs.contains(&canon) {
                continue;
            }
            prune_dir(assets_root, &canon, kept_files, kept_dirs);
            if std::fs::read_dir(&canon).is_ok_and(|mut it| it.next().is_none()) {
                let _ = std::fs::remove_dir(&canon);
            }
        } else if ft.is_file() && !kept_files.contains(&canon) {
            let _ = std::fs::remove_file(&canon);
        }
    }
}

// ---- Recent designs: the app's own list, kept in one small file ----

/// How many designs the list keeps.
pub const MAX_RECENTS: usize = 12;

/// A design opened or saved before, newest first.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recent {
    pub dir: String,
    /// Milliseconds since 1970 when it was last opened or saved.
    #[serde(default)]
    pub opened_ms: u64,
    /// Whether the folder still holds a design; worked out on reading, never stored.
    #[serde(default, skip_deserializing)]
    pub found: bool,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn read_recents(file: &Path) -> Vec<Recent> {
    std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_recents(file: &Path, list: &[Recent]) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
    std::fs::write(file, json).map_err(|e| e.to_string())
}

/// The list in `file`, each marked found or not.
pub fn recents(file: &Path) -> Vec<Recent> {
    let mut list = read_recents(file);
    for r in &mut list {
        r.found = folder_state(Path::new(&r.dir)) == Ok(FolderState::Design);
    }
    list
}

/// Put `dir` at the top of the list, as just opened or saved.
pub fn remember_recent(file: &Path, dir: &Path) -> Result<(), String> {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()).to_string_lossy().into_owned();
    let mut list = read_recents(file);
    list.retain(|r| r.dir != dir);
    list.insert(0, Recent { dir, opened_ms: now_ms(), found: true });
    list.truncate(MAX_RECENTS);
    write_recents(file, &list)
}

/// Take `dir` off the list; the folder itself is left alone.
pub fn forget_recent(file: &Path, dir: &str) -> Result<Vec<Recent>, String> {
    let mut list = read_recents(file);
    list.retain(|r| r.dir != dir);
    write_recents(file, &list)?;
    Ok(recents(file))
}

/// Point the entry for `old` at `new`, a design folder found elsewhere, keeping its place.
pub fn relocate_recent(file: &Path, old: &str, new: &Path) -> Result<Vec<Recent>, String> {
    if folder_state(new)? != FolderState::Design {
        return Err("这个文件夹不是设计".into());
    }
    let new = new.canonicalize().map_err(|e| e.to_string())?.to_string_lossy().into_owned();
    let mut list = read_recents(file);
    list.retain(|r| r.dir != new || r.dir == old);
    match list.iter_mut().find(|r| r.dir == old) {
        Some(r) => r.dir = new,
        None => list.insert(0, Recent { dir: new, opened_ms: now_ms(), found: true }),
    }
    write_recents(file, &list)?;
    Ok(recents(file))
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
        let pos = |id: &str| Position { id: id.into(), name: String::new(), phi_deg: 0.0, crop_deg: 30.0, crop_side: Default::default(), crop_offset_deg: 0.0, crops: None, neutral: None, order: vec![] };
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
            images: vec![SceneImage { id: "i".into(), name: "p".into(), width: 4, height: 4, position_id: "b".into(), colour: false, layer: Layer { path: picture.into(), ..Layer::plain() } }],
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

        // Saving again copies nothing more; a different file of the same name gets its own,
        // and the replaced copy is cleaned up.
        save(&design, &saved).unwrap();
        std::fs::write(outside.join("m.obj"), "v 1 1 1\n").unwrap();
        let again = save(&design, &s).unwrap();
        assert!(again.objects[0].path.ends_with("assets/m-2.obj"), "{}", again.objects[0].path);
        assert!(!design.join("assets/m.obj").exists());
        assert_eq!(std::fs::read_dir(design.join(ASSETS_DIR)).unwrap().count(), 3);

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

    #[test]
    fn unused_assets_are_removed_on_resave() {
        let outside = dir("prune-outside");
        std::fs::write(outside.join("p1.png"), [1u8, 2, 3]).unwrap();
        std::fs::write(outside.join("p2.png"), [4u8, 5, 6]).unwrap();
        std::fs::write(outside.join("g.bin"), [9u8; 8]).unwrap();
        std::fs::write(outside.join("g.gltf"), r#"{"buffers":[{"uri":"g.bin"}]}"#).unwrap();

        let mut s = scene(outside.join("g.gltf").to_str().unwrap(), outside.join("p1.png").to_str().unwrap());
        s.images.push(SceneImage {
            id: "i2".into(),
            name: "p2".into(),
            width: 4,
            height: 4,
            position_id: "b".into(),
            colour: false,
            layer: Layer { path: outside.join("p2.png").to_string_lossy().into(), ..Layer::plain() },
        });

        let design = dir("prune-design");
        let outside_note = design.join("notes.txt");
        std::fs::write(&outside_note, "keep me").unwrap();

        let mut saved = save(&design, &s).unwrap();
        let design = design.canonicalize().unwrap();
        assert!(design.join("assets/p1.png").exists());
        assert!(design.join("assets/p2.png").exists());
        assert!(design.join("assets/g/g.gltf").exists());
        assert!(design.join("assets/g/g.bin").exists());

        // An extra unused subfolder inside assets should be cleaned up once emptied.
        std::fs::create_dir_all(design.join("assets/stale")).unwrap();
        std::fs::write(design.join("assets/stale/old.png"), [0u8]).unwrap();

        // Remove the second picture and save again.
        saved.images.retain(|img| img.id != "i2");
        save(&design, &saved).unwrap();

        assert!(!design.join("assets/p2.png").exists());
        assert!(design.join("assets/p1.png").exists());
        assert!(design.join("assets/g/g.gltf").exists());
        assert!(design.join("assets/g/g.bin").exists());
        assert!(!design.join("assets/stale").exists());
        assert!(design.join(DESIGN_FILE).exists());
        assert_eq!(std::fs::read_to_string(design.join("notes.txt")).unwrap(), "keep me");

        for d in [outside, design] {
            std::fs::remove_dir_all(d).unwrap();
        }
    }

    #[test]
    fn folder_state_and_non_design_folder_error() {
        let root = dir("state");
        let missing = root.join("missing");
        assert_eq!(folder_state(&missing).unwrap(), FolderState::Missing);

        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(folder_state(&empty).unwrap(), FolderState::Empty);
        assert_eq!(open(&empty).unwrap_err(), "这个文件夹不是设计");

        let other = root.join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("readme.txt"), "hello").unwrap();
        assert_eq!(folder_state(&other).unwrap(), FolderState::Other);
        assert_eq!(open(&other).unwrap_err(), "这个文件夹不是设计");

        let regular_file = root.join("plain.txt");
        std::fs::write(&regular_file, "x").unwrap();
        assert_eq!(folder_state(&regular_file).unwrap(), FolderState::Other);

        let design_dir = root.join("design");
        std::fs::create_dir_all(&design_dir).unwrap();
        std::fs::write(design_dir.join(DESIGN_FILE), "{}").unwrap();
        assert_eq!(folder_state(&design_dir).unwrap(), FolderState::Design);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recents_keep_the_newest_first_and_can_be_relocated() {
        let root = dir("recents");
        let file = root.join("recent.json");
        let a = root.join("a");
        let b = root.join("b");
        for d in [&a, &b] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join(DESIGN_FILE), "{}").unwrap();
        }
        remember_recent(&file, &a).unwrap();
        remember_recent(&file, &b).unwrap();
        remember_recent(&file, &a).unwrap();
        let list = recents(&file);
        let a_str = a.canonicalize().unwrap().to_string_lossy().into_owned();
        let b_str = b.canonicalize().unwrap().to_string_lossy().into_owned();
        assert_eq!(list.iter().map(|r| r.dir.as_str()).collect::<Vec<_>>(), [a_str.as_str(), b_str.as_str()]);
        assert!(list.iter().all(|r| r.found));

        // Moved away: still listed, not found, then pointed at where it went.
        let moved = root.join("moved");
        std::fs::rename(&b, &moved).unwrap();
        assert!(!recents(&file)[1].found);
        assert!(relocate_recent(&file, &b_str, &root).is_err(), "not a design folder");
        let list = relocate_recent(&file, &b_str, &moved).unwrap();
        assert_eq!(list[1].dir, moved.canonicalize().unwrap().to_string_lossy());
        assert!(list[1].found);

        let list = forget_recent(&file, &a_str).unwrap();
        assert_eq!(list.len(), 1);
        assert!(moved.join(DESIGN_FILE).is_file(), "forgetting leaves the folder alone");
    }

    #[test]
    fn mask_crop_assets_are_adopted_resolved_and_pruned() {
        use crate::holo::model3d::{Crop, MaskChannel};

        let outside = dir("mask-outside");
        std::fs::write(outside.join("m1.png"), [10u8, 20, 30]).unwrap();
        std::fs::write(outside.join("m2.png"), [40u8, 50, 60]).unwrap();
        let mut s = scene(BUILTIN_SAMPLE_PATH, "");
        s.images.clear();
        s.objects.retain(|o| o.path == BUILTIN_SAMPLE_PATH);
        let make_mask = |id: &str, path: &Path| Crop::Mask {
            id: id.into(),
            name: id.into(),
            visible: true,
            path: path.to_string_lossy().into(),
            width: 8,
            height: 8,
            x: 0.0,
            y: 0.0,
            size: 1.0,
            stretch_x: 1.0,
            stretch_y: 1.0,
            rotation_deg: 0.0,
            channel: MaskChannel::Alpha,
            invert: false,
        };
        s.positions[0].crops = Some(vec![make_mask("c1", &outside.join("m1.png")), make_mask("c2", &outside.join("m2.png"))]);

        let design = dir("mask-design");
        save(&design, &s).unwrap();
        let design = design.canonicalize().unwrap();
        assert!(design.join("assets/m1.png").exists());
        assert!(design.join("assets/m2.png").exists());
        let written: serde_json::Value = serde_json::from_slice(&std::fs::read(design.join(DESIGN_FILE)).unwrap()).unwrap();
        assert_eq!(written["positions"][0]["crops"][0]["path"], "assets/m1.png");
        assert_eq!(written["positions"][0]["crops"][1]["path"], "assets/m2.png");

        // Moving the folder resolves mask paths cleanly.
        let moved = dir("mask-moved").join("d");
        std::fs::rename(&design, &moved).unwrap();
        let (opened, _) = open(&moved).unwrap();
        assert_eq!(std::fs::read(opened["positions"][0]["crops"][0]["path"].as_str().unwrap()).unwrap(), [10u8, 20, 30]);

        // Removing a mask crop and re-saving from the moved folder prunes the unused mask file.
        let mut reopened: Scene = serde_json::from_value(opened).unwrap();
        if let Some(crops) = &mut reopened.positions[0].crops {
            crops.retain(|c| c.mask_path().is_none_or(|p| p.ends_with("m1.png")));
        }
        let resaved = save(&moved, &reopened).unwrap();
        assert!(resaved.positions[0].crops.as_ref().unwrap().len() == 1);
        assert!(moved.join("assets/m1.png").exists());
        assert!(!moved.join("assets/m2.png").exists());

        for d in [outside, moved.parent().unwrap().to_path_buf()] {
            std::fs::remove_dir_all(d).unwrap();
        }
    }
}
