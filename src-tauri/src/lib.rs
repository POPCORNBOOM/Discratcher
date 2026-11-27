//! Tauri shell: thin commands over disc-core. Long jobs run on blocking threads and report
//! through events, so the window stays live.

use disc_core::artwork::{self, ArtworkAnalysis, Composition, Picture};
use disc_core::composition;
use disc_core::calib::disc_check::{self, DiscCheck, DiscKind};
use disc_core::calib::ladder::{self, CalibrationResult, LadderConfig};
use disc_core::carver::{self, CarveJobConfig, CarveJobSummary, Encoding, IsoAudit, Tone};
use disc_core::drive::{self, burner, OpticalDriveInfo};
use disc_core::iso9660;
use disc_core::media::Media;
use disc_core::preset::DiscGeometryPreset;
use disc_core::preview::{self, PreviewInput};
use disc_core::spiral::Spiral;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
struct Studio {
    cancel: Arc<AtomicBool>,
    /// Jobs holding a drive exclusively. Quitting waits for them to let go.
    holding: Arc<AtomicUsize>,
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

/// One calibration attempt, kept whatever its outcome; the rung timings can refit the ETA model.
#[tauri::command]
fn add_calibration_record(app: AppHandle, record: serde_json::Value) -> Result<(), String> {
    let mut log = calibration_log(&app)?;
    log.push(record);
    write_calibration_log(&app, &log)
}

/// Probing a drive costs tens of milliseconds, and the page asks every three
/// seconds, so it runs on a blocking thread rather than the UI one.
#[tauri::command]
async fn list_optical_drives(fresh: Option<bool>) -> Result<Vec<OpticalDriveInfo>, String> {
    blocking(move || {
        // A disc the drive struggles to read answers drutil in seconds, so a poll reads the
        // one already known; only a user's refresh or the end of a job asks the drive again.
        let drives = if fresh.unwrap_or(false) { drive::list_drives_fresh() } else { drive::list_drives() };
        // An empty list from a service that stopped answering is not "no drive".
        if drives.is_empty() && drive::stuck() {
            return Err("stuck".into());
        }
        // Nor is one from a drive busy with its disc: the page keeps what it last saw.
        if drives.is_empty() && drive::busy() {
            return Err("busy".into());
        }
        Ok(drives)
    })
    .await
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

/// One calibration, written where the user points, so it can be carried to another machine.
#[tauri::command]
fn export_preset(path: String, preset: DiscGeometryPreset) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(&preset).map_err(|e| e.to_string())?;
    write_atomic(Path::new(&path), &text)
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

/// Counts a job as holding the drive for as long as it lives. Declared before the drive, so
/// it is dropped after it: the count falls only once the drive is released and remounted.
struct Holding(Arc<AtomicUsize>);

impl Holding {
    fn new(state: &State<Studio>) -> Self {
        state.holding.fetch_add(1, Ordering::SeqCst);
        Self(state.holding.clone())
    }
}

impl Drop for Holding {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How long quitting waits for a job to release the drive. A timed read gives up after 30 s,
/// but the ladder checks for cancel between reads, which take milliseconds.
const RELEASE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Quitting while a job holds the drive would leave the drive held and the disc unmounted:
/// the tray then will not open until the drive is replugged. So the job is cancelled, the
/// window goes away at once, and the app exits once the drive is let go.
fn release_then_exit(app: &AppHandle) -> bool {
    let state = app.state::<Studio>();
    if state.holding.load(Ordering::SeqCst) == 0 {
        return false;
    }
    state.cancel.store(true, Ordering::Relaxed);
    for w in app.webview_windows().values() {
        let _ = w.hide();
    }
    let (app, holding) = (app.clone(), state.holding.clone());
    std::thread::spawn(move || {
        let began = std::time::Instant::now();
        while holding.load(Ordering::SeqCst) > 0 && began.elapsed() < RELEASE_WAIT {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // Counted as released from here on, so the exit this asks for goes through.
        holding.store(0, Ordering::SeqCst);
        app.exit(0);
    });
    true
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

/// The disc family, from the type the page already holds. Asking the drive again here
/// fails: for a few seconds after a pass-through session ends, macOS lists the drive
/// without its disc (seen on the GUD1N between the inspection and the check).
fn media_of(media_type: &str) -> Result<Media, String> {
    Media::of_type(media_type).ok_or_else(|| format!("目前只支持 DVD 和 CD 光盘，这张是 {media_type}"))
}

#[tauri::command]
async fn start_calibration(
    app: AppHandle,
    state: State<'_, Studio>,
    drive_id: String,
    media_type: String,
    max_targets: usize,
    reps: usize,
    range_pct: (f64, f64),
) -> Result<CalibrationResult, String> {
    let cancel = fresh_cancel(&state);
    let held = Holding::new(&state);
    blocking(move || {
        let _held = held;
        let media = media_of(&media_type)?;
        let mut d = drive::open_timed(&drive_id)?;
        let image_sectors = if media == Media::Cd { disc_check::calibration_sectors(d.as_mut()) } else { None };
        let cfg = LadderConfig { max_targets, reps, range_pct, media, image_sectors, ..Default::default() };
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

/// Opens the tray. Writes nothing.
#[tauri::command]
async fn eject_disc(drive_id: String) -> Result<(), String> {
    blocking(move || drive::eject(&drive_id)).await
}

/// What the drive holds, for deciding whether calibrating starts with a burn.
#[tauri::command]
async fn inspect_disc(state: State<'_, Studio>, drive_id: String) -> Result<DiscKind, String> {
    // Reading a CD takes it off the desktop until the read lets go: quitting waits for that.
    let held = Holding::new(&state);
    blocking(move || {
        let _held = held;
        disc_check::inspect(&drive_id)
    })
    .await
}

/// Reads a burned calibration disc back before it is timed. Only reads.
#[tauri::command]
async fn check_calibration_disc(app: AppHandle, state: State<'_, Studio>, drive_id: String, media_type: String) -> Result<DiscCheck, String> {
    let cancel = fresh_cancel(&state);
    let held = Holding::new(&state);
    blocking(move || {
        let _held = held;
        let media = media_of(&media_type)?;
        let mut d = drive::open_timed(&drive_id)?;
        disc_check::check_calibration_disc(d.as_mut(), media, &cancel, &mut |p| {
            let _ = app.emit("calib://check", p);
        })
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

/// Draws the twist chart for a CD calibration and returns the picture's path, to be
/// carved like any other picture. See `disc_core::twist`.
#[tauri::command]
async fn make_twist_chart(app: AppHandle, preset: DiscGeometryPreset) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("images");
    blocking(move || {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // Named by what it was drawn against: a new value draws a new chart.
        let id: String = preset.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        let path = dir.join(format!("twist-chart-{id}-{}.png", preset.along_track_frames.unwrap_or(0.0).round()));
        disc_core::twist::write_chart(&preset, &path)?;
        Ok(path.to_string_lossy().into_owned())
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
        let audit = carver::audit_iso(Path::new(&summary.output_iso_path), &Encoding::of(&config))?;
        Ok(BuiltIso { summary, audit })
    })
    .await
}

/// Experimental: the codebook calibration disc image. Its layout is kept in the app's data
/// folder, since the photos of the burned disc are read against it later.
#[tauri::command]
async fn holo_build_swatch_iso(
    app: AppHandle,
    state: State<'_, Studio>,
    preset: DiscGeometryPreset,
    output_iso_path: String,
) -> Result<disc_core::holo::swatch::SwatchSummary, String> {
    let cancel = fresh_cancel(&state);
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("holo");
    blocking(move || {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let layout_path = dir.join(format!("swatch-{stamp}.json"));
        disc_core::holo::swatch::build_iso(&preset, &output_iso_path, &layout_path, &cancel, &|p| {
            let _ = app.emit("carve://progress", p);
        })
    })
    .await
}

/// The last design solved, so dragging the preview angle does not solve it again.
fn solved_design(preset: &DiscGeometryPreset, input: &disc_core::holo::design::DesignInput, uniform_background: bool) -> Result<Arc<disc_core::holo::design::Solved>, String> {
    use disc_core::holo::design;
    static LAST: Mutex<Option<(String, Arc<design::Solved>)>> = Mutex::new(None);
    let key = serde_json::to_string(&(preset, input, uniform_background)).map_err(|e| e.to_string())?;
    if let Some((k, v)) = LAST.lock().unwrap().as_ref() {
        if *k == key {
            return Ok(v.clone());
        }
    }
    let shared = design::shared_codebook(preset.channel_bit_length_nm * 1e-3);
    let mut solved = design::solve(input, &shared.1, preset)?;
    if uniform_background {
        design::uniform_background(&mut solved, input, &shared.0)?;
    }
    let solved = Arc::new(solved);
    *LAST.lock().unwrap() = Some((key, solved.clone()));
    Ok(solved)
}

/// Experimental: the disc as seen from `view`, RGBA, `size` square.
#[tauri::command]
async fn holo_design_preview(
    preset: DiscGeometryPreset,
    input: disc_core::holo::design::DesignInput,
    view: disc_core::holo::design::ViewSpec,
    size: usize,
    uniform_background: Option<bool>,
) -> Result<tauri::ipc::Response, String> {
    blocking(move || {
        let solved = solved_design(&preset, &input, uniform_background.unwrap_or(false))?;
        let shared = disc_core::holo::design::shared_codebook(preset.channel_bit_length_nm * 1e-3);
        Ok(tauri::ipc::Response::new(disc_core::holo::design::render(&solved, &shared.1, &preset, &view, size.clamp(64, 1024), 1.0)))
    })
    .await
}

#[tauri::command]
async fn holo_build_design_iso(
    app: AppHandle,
    state: State<'_, Studio>,
    preset: DiscGeometryPreset,
    input: disc_core::holo::design::DesignInput,
    output_iso_path: String,
    uniform_background: Option<bool>,
) -> Result<disc_core::holo::design::DesignSummary, String> {
    let cancel = fresh_cancel(&state);
    blocking(move || {
        let solved = solved_design(&preset, &input, uniform_background.unwrap_or(false))?;
        let shared = disc_core::holo::design::shared_codebook(preset.channel_bit_length_nm * 1e-3);
        disc_core::holo::design::build_iso(&preset, &shared.0, &solved, None, &output_iso_path, &cancel, &|p| {
            let _ = app.emit("carve://progress", p);
        })
    })
    .await
}

/// Experimental: writes the 4-sphere 3D parallax test scene targets into the app's data
/// folder and returns the matching fixed-lamp horizontal head-motion views.
#[tauri::command]
async fn holo_test3d_views(
    app: AppHandle,
    preset: Option<DiscGeometryPreset>,
) -> Result<Vec<disc_core::holo::design::ViewSpec>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("holo").join("test3d");
    blocking(move || {
        let p = preset
            .filter(|p| p.media == Media::Dvd)
            .unwrap_or_else(disc_core::preset::reference_preset);
        disc_core::holo::testscene::write_test3d_views(&dir, &p)
    })
    .await
}

/// Load a 3D mesh (OBJ / glTF / GLB or `builtin:star`) into a normalized, size-limited binary
/// buffer for the 3D scene editor viewport: `[v_count:u32][i_count:u32][positions][normals][indices]`.
#[tauri::command]
async fn holo_load_mesh(path: String) -> Result<tauri::ipc::Response, String> {
    blocking(move || {
        let mesh = disc_core::holo::model3d::load_mesh_or_builtin(&path)?;
        disc_core::holo::model3d::mesh_to_display_bytes(&mesh)
    })
    .await
    .map(tauri::ipc::Response::new)
}

fn solved_scene(
    scene: &disc_core::holo::model3d::Scene,
    options: &disc_core::holo::turntable::Options,
    preset: &DiscGeometryPreset,
) -> Result<disc_core::holo::turntable::Solution, String> {
    use disc_core::holo::{design, turntable};
    static LAST: Mutex<Option<(String, turntable::Solution)>> = Mutex::new(None);
    let key = serde_json::to_string(&(scene, options, preset)).map_err(|e| e.to_string())?;
    if let Some((k, v)) = LAST.lock().unwrap().as_ref() {
        if *k == key {
            return Ok(v.clone());
        }
    }
    let shared = design::shared_codebook(preset.channel_bit_length_nm * 1e-3);
    let s = turntable::solve(scene, options, &shared.0, &shared.1, preset)?;
    *LAST.lock().unwrap() = Some((key, s.clone()));
    Ok(s)
}

/// Solves a scene of objects, each about its own axis, and says which overlap.
#[tauri::command]
async fn holo_solve_scene(
    scene: disc_core::holo::model3d::Scene,
    options: disc_core::holo::turntable::Options,
    preset: DiscGeometryPreset,
) -> Result<disc_core::holo::turntable::Solution, String> {
    blocking(move || solved_scene(&scene, &options, &preset)).await
}

/// The solved scene's weights, for the page to draw on the graphics card.
#[tauri::command]
async fn holo_scene_weights(
    scene: disc_core::holo::model3d::Scene,
    options: disc_core::holo::turntable::Options,
    preset: DiscGeometryPreset,
    tone: FlatTone,
) -> Result<tauri::ipc::Response, String> {
    blocking(move || {
        let s = solved_scene(&scene, &options, &preset)?;
        // Pictures are drawn at the solve grid's size, so a long edge of 1024 is plenty.
        let flat = if s.flat.positions.is_empty() {
            vec![]
        } else {
            let tone = disc_core::carver::Tone::from_duty(tone.servo_safe_dither, tone.max_dark_duty_cycle);
            disc_core::holo::flat::Flat::load(s.flat.clone(), &scene, Some(1024))?.preview_layer(tone)
        };
        Ok(tauri::ipc::Response::new(disc_core::holo::turntable::weight_layers(&s.solved, &flat)))
    })
    .await
}

/// What each sequence looks like by angle, for the page to draw on the graphics card.
#[tauri::command]
async fn holo_response_tables(preset: DiscGeometryPreset) -> Result<tauri::ipc::Response, String> {
    blocking(move || {
        let shared = disc_core::holo::design::shared_codebook(preset.channel_bit_length_nm * 1e-3);
        let carve = disc_core::holo::design::carve_looks(preset.channel_bit_length_nm * 1e-3);
        Ok(tauri::ipc::Response::new(disc_core::holo::turntable::response_tables(&shared.1, &carve, preset.base_pitch_um)))
    })
    .await
}

/// How flat pictures are dithered: the carve page's own settings.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlatTone {
    servo_safe_dither: bool,
    max_dark_duty_cycle: f64,
}

#[tauri::command]
async fn holo_build_scene_iso(
    app: AppHandle,
    state: State<'_, Studio>,
    scene: disc_core::holo::model3d::Scene,
    options: disc_core::holo::turntable::Options,
    preset: DiscGeometryPreset,
    tone: FlatTone,
    output_iso_path: String,
) -> Result<disc_core::holo::design::DesignSummary, String> {
    let cancel = fresh_cancel(&state);
    blocking(move || {
        let s = solved_scene(&scene, &options, &preset)?;
        let shared = disc_core::holo::design::shared_codebook(preset.channel_bit_length_nm * 1e-3);
        // Flat pictures at full size, as the carve page burns them.
        let flat = if s.flat.positions.is_empty() { None } else { Some(disc_core::holo::flat::Flat::load(s.flat.clone(), &scene, None)?) };
        let tone = disc_core::carver::Tone::from_duty(tone.servo_safe_dither, tone.max_dark_duty_cycle);
        disc_core::holo::design::build_iso(&preset, &shared.0, &s.solved, flat.as_ref().map(|f| (f, tone)), &output_iso_path, &cancel, &|p| {
            let _ = app.emit("carve://progress", p);
        })
    })
    .await
}

/// Save a design into its folder, bringing every file it uses inside. Returns the scene with its
/// paths pointing at those copies.
#[tauri::command]
async fn design_save(dir: String, scene: disc_core::holo::model3d::Scene) -> Result<disc_core::holo::model3d::Scene, String> {
    blocking(move || disc_core::holo::project::save(Path::new(&dir), &scene)).await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenedDesign {
    scene: disc_core::holo::model3d::Scene,
    /// The design's folder; none for a scene file from before designs were folders.
    dir: Option<String>,
}

/// Open a design folder, its `design.json`, or an older scene file.
#[tauri::command]
async fn design_open(path: String) -> Result<OpenedDesign, String> {
    blocking(move || {
        let (scene, dir) = disc_core::holo::project::open(Path::new(&path))?;
        Ok(OpenedDesign { scene, dir: dir.map(|d| d.to_string_lossy().into()) })
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
) -> Result<(), String> {
    let cancel = fresh_cancel(&state);
    // Quitting mid-burn stops the burner and waits for it, rather than leave it writing alone.
    let held = Holding::new(&state);
    blocking(move || {
        let _held = held;
        disc_core::drive::check_burnable(&drive_id, &iso_path)?;
        // Windows burns inside the app: IMAPI2 is called straight from Rust, so progress
        // arrives from the drive and no window or helper process is involved.
        #[cfg(target_os = "windows")]
        {
            let _ = verify;
            let drive = disc_core::drive::windows::drive_letter(&drive_id)
                .ok_or_else(|| format!("{drive_id} 没有盘符，无法交给刻录机"))?;
            let reporter = app.clone();
            return disc_core::drive::imapi::burn(&drive, &iso_path, speed, cancel, move |percent, phase| {
                let p = burner::BurnProgress { percent: Some(percent), message: phase.to_string(), ..Default::default() };
                let _ = reporter.emit("burn://progress", p);
            });
        }
        #[cfg(not(target_os = "windows"))]
        {
            let argv = burner::burn_command(&drive_id, &iso_path, speed, verify)?;
            let mut emit = |p: burner::BurnProgress| {
                let _ = app.emit("burn://progress", p);
            };
            // A written rewritable disc is erased first, here and nowhere else: the page
            // warned about it before the user pressed the burn button. It is a step of its
            // own, so its progress is never taken for the write's.
            // Asked for now: a disc a failed burn left written has to be erased first.
            let erasable = disc_core::drive::list_drives_fresh()
                .into_iter()
                .any(|d| d.id == drive_id && d.disc == disc_core::drive::DiscUse::Erasable);
            if erasable {
                burner::erase(&burner::erase_command(&drive_id)?, &cancel, &mut emit)?;
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err("已取消".into());
                }
            }
            burner::burn(&argv, &cancel, &mut emit)
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
            export_preset,
            add_calibration_record,
            cancel_job,
            generate_calibration_iso,
            start_calibration,
            inspect_disc,
            eject_disc,
            check_calibration_disc,
            analyze_artwork,
            simulate_disc_preview,
            preview_texture,
            save_composition,
            open_composition,
            build_carved_iso,
            make_twist_chart,
            burn_iso_to_drive,
            holo_build_swatch_iso,
            holo_design_preview,
            holo_build_design_iso,
            holo_test3d_views,
            holo_load_mesh,
            holo_solve_scene,
            holo_scene_weights,
            holo_response_tables,
            holo_build_scene_iso,
            design_save,
            design_open,
            cache_iso_path,
            remove_cached_iso,
        ])
        .on_window_event(|w, e| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = e {
                if release_then_exit(w.app_handle()) {
                    api.prevent_close();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, e| {
            if let tauri::RunEvent::ExitRequested { api, .. } = e {
                if release_then_exit(app) {
                    api.prevent_exit();
                }
            }
        });
}