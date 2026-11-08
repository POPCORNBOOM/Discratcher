//! Tauri shell: thin commands over disc-core. Long jobs run on blocking threads and report
//! through events, so the window stays live.

use disc_core::artwork::{self, ArtworkAnalysis, Composition, Picture};
use disc_core::composition;
use disc_core::calib::ladder::{self, CalibrationResult, LadderConfig};
use disc_core::carver::{self, CarveJobConfig, CarveJobSummary, IsoAudit, Tone};
use disc_core::drive::{self, burner, OpticalDriveInfo};
use disc_core::iso9660;
use disc_core::preset::DiscGeometryPreset;
use disc_core::preview::{self, PreviewInput};
use disc_core::spiral::Spiral;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
struct Studio {
    cancel: Arc<AtomicBool>,
    /// Pictures decoded for the fallback preview, by path.
    pictures: Mutex<std::collections::HashMap<String, Arc<Picture>>>,
}

/// Long edge of the pictures the previews work from.
const PREVIEW_EDGE: u32 = 1024;

fn data_file(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(name))
}

fn presets_path(app: &AppHandle) -> Result<PathBuf, String> {
    data_file(app, "presets.json")
}

/// Temp file then rename: a crash mid-write must not cost hours of measuring.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn user_presets(app: &AppHandle) -> Result<Vec<DiscGeometryPreset>, String> {
    match std::fs::read(presets_path(app)?) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("预设文件已损坏：{e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

fn write_presets(app: &AppHandle, v: &[DiscGeometryPreset]) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?;
    write_atomic(&presets_path(app)?, &text)
}

fn calibration_log(app: &AppHandle) -> Result<Vec<serde_json::Value>, String> {
    match std::fs::read(data_file(app, "calibrations.json")?) {
        Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("标定记录已损坏：{e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

fn write_calibration_log(app: &AppHandle, log: &[serde_json::Value]) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(log).map_err(|e| e.to_string())?;
    write_atomic(&data_file(app, "calibrations.json")?, &text)
}

/// Newest first. The shape belongs to the page that writes it; this only keeps it safe.
#[tauri::command]
fn get_calibration_log(app: AppHandle) -> Result<Vec<serde_json::Value>, String> {
    let mut log = calibration_log(&app)?;
    log.reverse();
    Ok(log)
}

#[tauri::command]
fn add_calibration_record(app: AppHandle, record: serde_json::Value) -> Result<(), String> {
    let mut log = calibration_log(&app)?;
    log.push(record);
    write_calibration_log(&app, &log)
}

#[tauri::command]
fn delete_calibration_record(app: AppHandle, id: String) -> Result<(), String> {
    let mut log = calibration_log(&app)?;
    log.retain(|r| r["id"].as_str() != Some(id.as_str()));
    write_calibration_log(&app, &log)
}

/// Probing a drive costs tens of milliseconds, and the page asks every three
/// seconds, so it runs on a blocking thread rather than the UI one.
#[tauri::command]
async fn list_optical_drives() -> Vec<OpticalDriveInfo> {
    blocking(|| Ok(drive::list_drives())).await.unwrap_or_default()
}

#[tauri::command]
fn get_presets(app: AppHandle) -> Result<Vec<DiscGeometryPreset>, String> {
    user_presets(&app)
}

#[tauri::command]
fn save_preset(app: AppHandle, preset: DiscGeometryPreset) -> Result<(), String> {
    // Downloaded presets arrive from outside: unusable geometry stops here.
    Spiral::new(preset.spiral_params())?;
    let mut v = user_presets(&app)?;
    match v.iter_mut().find(|p| p.id == preset.id) {
        Some(p) => *p = preset,
        None => v.push(preset),
    }
    write_presets(&app, &v)
}

#[tauri::command]
fn delete_preset(app: AppHandle, id: String) -> Result<(), String> {
    let mut v = user_presets(&app)?;
    v.retain(|p| p.id != id);
    write_presets(&app, &v)
}

#[tauri::command]
fn cancel_job(state: State<Studio>) {
    state.cancel.store(true, Ordering::Relaxed);
}

fn fresh_cancel(state: &State<Studio>) -> Arc<AtomicBool> {
    state.cancel.store(false, Ordering::Relaxed);
    state.cancel.clone()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

fn iso_cache_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("burn");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Where a full-disc image lives between composing and burning.
#[tauri::command]
fn cache_iso_path(app: AppHandle, name: String) -> Result<String, String> {
    let file = Path::new(&name).file_name().ok_or("镜像文件名无效")?;
    Ok(iso_cache_dir(&app)?.join(file).to_string_lossy().into_owned())
}

/// A 4.7 GB leftover is worth removing, but only ever from our own cache folder.
#[tauri::command]
fn remove_cached_iso(app: AppHandle, path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if path.parent() != Some(iso_cache_dir(&app)?.as_path()) {
        return Err("只能删除缓存目录里的镜像".into());
    }
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

#[tauri::command]
async fn generate_calibration_iso(app: AppHandle, output_path: String, total_sectors: u32) -> Result<(), String> {
    blocking(move || {
        iso9660::generate_calibration_iso(Path::new(&output_path), total_sectors, &|p| {
            let _ = app.emit("carve://progress", p);
        })
    })
    .await
}

#[tauri::command]
async fn start_calibration(
    app: AppHandle,
    state: State<'_, Studio>,
    drive_id: String,
    max_targets: usize,
    reps: usize,
    range_pct: (f64, f64),
) -> Result<CalibrationResult, String> {
    let cancel = fresh_cancel(&state);
    blocking(move || {
        let mut d = drive::open_timed(&drive_id, true)?;
        let cfg = LadderConfig { max_targets, reps, range_pct, ..Default::default() };
        let a = app.clone();
        ladder::run_ladder(
            d.as_mut(),
            &cfg,
            &cancel,
            &mut |e| {
                let _ = a.emit("calib://rung-progress", e);
            },
            &mut |m| {
                let _ = app.emit("calib://status", m);
            },
        )
    })
    .await
}

#[tauri::command]
async fn analyze_artwork(image_path: String) -> Result<ArtworkAnalysis, String> {
    blocking(move || artwork::analyze(Path::new(&image_path))).await
}

fn preview_art(state: &Studio, cfg: &CarveJobConfig) -> Result<Composition, String> {
    let mut cache = state.pictures.lock().unwrap();
    cache.retain(|path, _| cfg.layers.iter().any(|l| &l.path == path));
    let mut layers = Vec::with_capacity(cfg.layers.len());
    for l in &cfg.layers {
        let picture = match cache.get(&l.path) {
            Some(p) => p.clone(),
            None => {
                let p = Arc::new(Picture::load(Path::new(&l.path), Some(PREVIEW_EDGE))?);
                cache.insert(l.path.clone(), p.clone());
                p
            }
        };
        layers.push((l.clone(), picture));
    }
    Ok(Composition::new(layers))
}

/// The native preview, for where the window has no WebGL.
#[tauri::command]
async fn simulate_disc_preview(
    app: AppHandle,
    config: CarveJobConfig,
    preview_size: u32,
) -> Result<String, String> {
    blocking(move || {
        let state = app.state::<Studio>();
        let art = preview_art(&state, &config)?;
        let spiral = Spiral::new(config.preset.spiral_params())?;
        let tone = Tone::from_duty(config.servo_safe_dither, config.max_dark_duty_cycle);
        preview::render_data_url(&PreviewInput {
            design: &spiral,
            actual: &spiral,
            art: &art,
            min_luma: if tone.servo_safe { tone.min_luma } else { 0.0 },
            size: preview_size.clamp(128, 2048),
        })
    })
    .await
}

/// A picture for the live preview as raw bytes (see `artwork::preview_texture`); the disc
/// renderer places and tones it every frame.
#[tauri::command]
async fn preview_texture(image_path: String) -> Result<tauri::ipc::Response, String> {
    blocking(move || artwork::preview_texture(Path::new(&image_path), PREVIEW_EDGE))
        .await
        .map(tauri::ipc::Response::new)
}

#[tauri::command]
async fn save_composition(path: String, layers: Vec<serde_json::Map<String, serde_json::Value>>) -> Result<(), String> {
    blocking(move || {
        let bytes = composition::pack(&layers)?;
        std::fs::write(&path, bytes).map_err(|e| e.to_string())
    })
    .await
}

/// The pictures are copied into the app's own folder, so the layers stay valid after
/// the composition file moves.
#[tauri::command]
async fn open_composition(app: AppHandle, path: String) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, String> {
    let images = app.path().app_data_dir().map_err(|e| e.to_string())?.join("images");
    blocking(move || {
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        composition::unpack(&bytes, &images)
    })
    .await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BuiltIso {
    summary: CarveJobSummary,
    audit: IsoAudit,
}

#[tauri::command]
async fn build_carved_iso(app: AppHandle, state: State<'_, Studio>, config: CarveJobConfig) -> Result<BuiltIso, String> {
    let cancel = fresh_cancel(&state);
    blocking(move || {
        let summary = carver::build_iso(&config, &cancel, &|p| {
            let _ = app.emit("carve://progress", p);
        })?;
        let audit = carver::audit_iso(Path::new(&summary.output_iso_path))?;
        Ok(BuiltIso { summary, audit })
    })
    .await
}

#[tauri::command]
async fn burn_iso_to_drive(
    app: AppHandle,
    state: State<'_, Studio>,
    drive_id: String,
    iso_path: String,
    speed: u8,
    verify: bool,
    test_only: bool,
) -> Result<(), String> {
    let cancel = fresh_cancel(&state);
    blocking(move || {
        if drive_id.starts_with("sim://") {
            // Nothing here touches a disc or a drive: it only plays the progress.
            return burner::burn_simulated(std::time::Duration::from_millis(60), &cancel, &mut |p| {
                let _ = app.emit("burn://progress", p);
            });
        }
        // Windows burns inside the app: IMAPI2 is called straight from Rust, so progress
        // arrives from the drive and no window or helper process is involved.
        #[cfg(target_os = "windows")]
        {
            if test_only {
                return Err("Windows 的刻录接口不支持试刻".into());
            }
            let _ = verify;
            let drive = disc_core::drive::windows::drive_letter(&drive_id)
                .ok_or_else(|| format!("{drive_id} 没有盘符，无法交给刻录机"))?;
            let reporter = app.clone();
            return disc_core::drive::imapi::burn(&drive, &iso_path, speed, cancel, move |percent, phase| {
                let p = burner::BurnProgress { percent: Some(percent), message: phase.to_string() };
                let _ = reporter.emit("burn://progress", p);
            });
        }
        #[cfg(not(target_os = "windows"))]
        {
            let argv = burner::burn_command(&drive_id, &iso_path, speed, verify, test_only)?;
            burner::burn(&argv, &cancel, &mut |p| {
                let _ = app.emit("burn://progress", p);
            })
        }
    })
    .await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Studio::default())
        .invoke_handler(tauri::generate_handler![
            list_optical_drives,
            get_presets,
            save_preset,
            delete_preset,
            get_calibration_log,
            add_calibration_record,
            delete_calibration_record,
            cancel_job,
            generate_calibration_iso,
            start_calibration,
            analyze_artwork,
            simulate_disc_preview,
            preview_texture,
            save_composition,
            open_composition,
            build_carved_iso,
            burn_iso_to_drive,
            cache_iso_path,
            remove_cached_iso,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}