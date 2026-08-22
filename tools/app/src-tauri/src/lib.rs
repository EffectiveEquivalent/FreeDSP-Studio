mod dsp;
mod hid;

#[cfg(windows)]
mod hid_win;

#[cfg(not(windows))]
mod hid_hidapi;

use std::fs;
use tauri::Manager;

// Saved EQs live as a plain JSON file in the per-OS app config dir (cross-platform:
// %APPDATA%\<id>\ on Windows, ~/Library/Application Support/<id>/ on macOS).
fn saved_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("saved-eqs.json"))
}

#[tauri::command]
fn saved_load(app: tauri::AppHandle) -> Result<String, String> {
    let p = saved_path(&app)?;
    Ok(fs::read_to_string(&p).unwrap_or_else(|_| "{}".to_string()))
}

#[tauri::command]
fn saved_save(app: tauri::AppHandle, data: String) -> Result<(), String> {
    let p = saved_path(&app)?;
    fs::write(&p, data).map_err(|e| e.to_string())
}

// Open a URL in the user's default browser (About-dialog links). Cross-platform.
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    open::that(url).map_err(|e| e.to_string())
}

// Saved frequency-response curves (imported measurements + targets), same config dir.
fn frs_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("saved-frs.json"))
}

#[tauri::command]
fn frs_load(app: tauri::AppHandle) -> Result<String, String> {
    let p = frs_path(&app)?;
    Ok(fs::read_to_string(&p).unwrap_or_else(|_| "{}".to_string()))
}

#[tauri::command]
fn frs_save(app: tauri::AppHandle, data: String) -> Result<(), String> {
    let p = frs_path(&app)?;
    fs::write(&p, data).map_err(|e| e.to_string())
}

// Write a combined backup (saved EQs + FR curves) to a user-chosen path.
#[tauri::command]
fn backup_write(path: String, data: String) -> Result<(), String> {
    fs::write(&path, data).map_err(|e| e.to_string())
}

// Run the blocking HID work off the UI thread so writes (~1s of frames) never freeze the window.
async fn blk<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn open() -> Result<hid::CableInfo, String> {
    blk(hid::open).await?
}

#[tauri::command]
async fn close() -> Result<(), String> {
    blk(hid::close).await
}

#[tauri::command]
async fn read_mode() -> Result<Option<i32>, String> {
    blk(hid::read_mode).await?
}

#[tauri::command]
async fn read_bank() -> Result<Vec<hid::Band>, String> {
    blk(hid::read_bank).await?
}

#[tauri::command]
async fn set_preset(idx: i32) -> Result<Option<i32>, String> {
    blk(move || hid::set_preset(idx)).await?
}

#[tauri::command]
async fn write_bank(bands: Vec<hid::BandIn>, preamp: f64) -> Result<hid::WriteResult, String> {
    blk(move || hid::write_bank(bands, preamp)).await?
}

#[tauri::command]
fn win_minimize(window: tauri::WebviewWindow) {
    let _ = window.minimize();
}

#[tauri::command]
fn win_close(window: tauri::WebviewWindow) {
    let _ = window.close();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            open,
            close,
            read_mode,
            read_bank,
            set_preset,
            write_bank,
            win_minimize,
            win_close,
            saved_load,
            saved_save,
            open_url,
            frs_load,
            frs_save,
            backup_write
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
