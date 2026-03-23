#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
  collections::{HashMap, VecDeque},
  fs,
  io::Read,
  path::{Path, PathBuf},
  sync::{Arc, Mutex},
  time::{Duration, Instant, UNIX_EPOCH},
};
use tauri::{
  AppHandle, Emitter, LogicalSize, Manager, PhysicalSize, Size, State,
  WebviewWindow, WindowEvent,
};
use tokio::{
  sync::{mpsc, watch},
  time::{interval, sleep},
};
use walkdir::WalkDir;

const DEFAULT_ORIGIN: &str = "https://heygrats.com";
const APP_WIDTH: f64 = 1120.0;
const APP_HEIGHT: f64 = 940.0;
const APP_MIN_WIDTH: f64 = 980.0;
const APP_MIN_HEIGHT: f64 = 820.0;
const APP_ASPECT_RATIO: f64 = APP_WIDTH / APP_HEIGHT;
const MAX_FILE_SIZE_BYTES: u64 = 50 * 1024 * 1024;
const FILE_STABLE_FOR_SECONDS: u64 = 2;
const FILE_STABILITY_TIMEOUT_SECONDS: u64 = 30;

#[derive(Default)]
struct SyncRuntime {
  active: Mutex<Option<ActiveSync>>,
  status: Arc<Mutex<SyncStatus>>,
}

struct ActiveSync {
  stop_tx: watch::Sender<bool>,
  task: tauri::async_runtime::JoinHandle<()>,
}

struct ResizeLockState {
  adjusting: bool,
  last_width: f64,
  last_height: f64,
}

impl Default for ResizeLockState {
  fn default() -> Self {
    Self {
      adjusting: false,
      last_width: APP_WIDTH,
      last_height: APP_HEIGHT,
    }
  }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct SyncStatus {
  running: bool,
  event_code: Option<String>,
  folder: Option<String>,
  last_message: Option<String>,
  last_error: Option<String>,
  files_synced: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartSyncInput {
  origin: String,
  token: String,
  folder: String,
  device_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionStatusResponse {
  active: bool,
  status: String,
  prompt_cleanup: Option<bool>,
  event: Option<EventSummary>,
}

#[derive(Debug, Clone, Deserialize)]
struct EventSummary {
  code: Option<String>,
  title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SignedUploadResponse {
  duplicate: Option<bool>,
  path: Option<String>,
  #[serde(rename = "signedUrl")]
  signed_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LocalCache {
  uploaded: HashMap<String, CachedUpload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedUpload {
  file_path: String,
  remote_path: String,
  uploaded_at: String,
}

#[derive(Debug, Clone, Serialize)]
struct LogPayload {
  message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CleanupPayload {
  token: String,
  message: String,
}

#[tauri::command]
async fn start_sync(
  app: AppHandle,
  runtime: State<'_, SyncRuntime>,
  input: StartSyncInput,
) -> Result<SyncStatus, String> {
  normalize_origin(&input.origin)?;
  let token = normalize_sync_token(&input.token);
  let folder = input.folder.trim().to_string();

  if token.is_empty() || folder.is_empty() {
    return Err("Token and folder are required.".into());
  }

  let folder_path = PathBuf::from(&folder);
  if !folder_path.is_dir() {
    return Err(format!("Folder not found: {}", folder));
  }

  stop_existing_sync(&runtime).await;

  let (stop_tx, stop_rx) = watch::channel(false);
  let status = runtime.status.clone();
  {
    let mut current = status.lock().map_err(|_| "Status lock poisoned")?;
    current.running = true;
    current.folder = Some(folder.clone());
    current.last_error = None;
    current.last_message = Some("Starting local sync...".into());
  }
  emit_status(&app, &status);

  let task_app = app.clone();
  let task_token = token.clone();
  let task_input = input.clone();
  let task = tauri::async_runtime::spawn(async move {
    run_sync(task_app, status, task_input, task_token, stop_rx).await;
  });

  let mut active = runtime.active.lock().map_err(|_| "Runtime lock poisoned")?;
  *active = Some(ActiveSync { stop_tx, task });

  Ok(snapshot_status(&runtime.status))
}

#[tauri::command]
async fn stop_sync(
  app: AppHandle,
  runtime: State<'_, SyncRuntime>,
) -> Result<SyncStatus, String> {
  stop_existing_sync(&runtime).await;
  {
    let mut status = runtime.status.lock().map_err(|_| "Status lock poisoned")?;
    status.running = false;
    status.last_message = Some("Sync stopped.".into());
    status.last_error = None;
  }
  emit_status(&app, &runtime.status);
  emit_log(&app, "Sync stopped from the desktop app.");
  Ok(snapshot_status(&runtime.status))
}

#[tauri::command]
fn get_sync_status(runtime: State<'_, SyncRuntime>) -> SyncStatus {
  snapshot_status(&runtime.status)
}

#[tauri::command]
fn clear_sync_cache(app: AppHandle, token: String) -> Result<(), String> {
  let token = normalize_sync_token(&token);
  if token.is_empty() {
    return Err("Token is required to clear local cache.".into());
  }
  let path = cache_file_path(&app, &token)?;
  if path.exists() {
    fs::remove_file(path).map_err(|err| err.to_string())?;
  }
  Ok(())
}

async fn stop_existing_sync(runtime: &State<'_, SyncRuntime>) {
  if let Ok(mut active) = runtime.active.lock() {
    if let Some(current) = active.take() {
      let _ = current.stop_tx.send(true);
      current.task.abort();
    }
  }
}

async fn run_sync(
  app: AppHandle,
  status: Arc<Mutex<SyncStatus>>,
  input: StartSyncInput,
  token: String,
  mut stop_rx: watch::Receiver<bool>,
) {
  let origin = match normalize_origin(&input.origin) {
    Ok(value) => value,
    Err(error) => {
      set_error(&app, &status, &error);
      return;
    }
  };
  let folder = match fs::canonicalize(input.folder.trim()) {
    Ok(value) => value,
    Err(error) => {
      set_error(
        &app,
        &status,
        &format!("Unable to resolve watched folder: {}", error),
      );
      return;
    }
  };
  if !folder.is_dir() {
    set_error(
      &app,
      &status,
      &format!("Watched folder is not a directory: {}", folder.display()),
    );
    return;
  }
  let device_name = if input.device_name.trim().is_empty() {
    default_device_name()
  } else {
    input.device_name.trim().to_string()
  };

  let client = reqwest::Client::new();
  let cache_path = match cache_file_path(&app, &token) {
    Ok(path) => path,
    Err(error) => {
      set_error(&app, &status, &error);
      return;
    }
  };

  let mut cache = load_cache(&cache_path);
  let session = match check_session(&client, &origin, &token, &device_name).await {
    Ok(response) => response,
    Err(error) => {
      set_error(&app, &status, &error);
      return;
    }
  };

  if !session.active {
    end_for_cleanup(&app, &status, &token, &session);
    return;
  }

  if let Ok(mut current) = status.lock() {
    current.event_code = session
      .event
      .as_ref()
      .and_then(|event| event.code.clone());
    current.last_message = Some(format!(
      "Watching {} for {}",
      folder.display(),
      display_event_name(session.event.as_ref())
    ));
  }
  emit_status(&app, &status);
  emit_log(
    &app,
    &format!(
      "Watching {} for {}.",
      folder.display(),
      display_event_name(session.event.as_ref())
    ),
  );

  let (path_tx, mut path_rx) = mpsc::unbounded_channel::<PathBuf>();
  enqueue_initial_scan(&folder, &path_tx);

  let mut watcher = match create_watcher(path_tx.clone()) {
    Ok(watcher) => watcher,
    Err(error) => {
      set_error(&app, &status, &format!("Failed to watch folder: {}", error));
      return;
    }
  };

  if let Err(error) = watcher.watch(&folder, RecursiveMode::Recursive) {
    set_error(&app, &status, &format!("Failed to watch folder: {}", error));
    return;
  }

  let mut queue: VecDeque<PathBuf> = VecDeque::new();
  let mut poll = interval(Duration::from_secs(20));

  loop {
    if let Some(next_path) = queue.pop_front() {
      if let Err(error) = process_file(
        &app,
        &client,
        &origin,
        &token,
        &device_name,
        &folder,
        &next_path,
        &mut cache,
        &cache_path,
        &status,
      )
      .await
      {
        emit_log(&app, &format!("Skip {}: {}", next_path.display(), error));
      }
      continue;
    }

    tokio::select! {
      _ = stop_rx.changed() => {
        if *stop_rx.borrow() {
          if let Ok(mut current) = status.lock() {
            current.running = false;
            current.last_message = Some("Sync stopped.".into());
          }
          emit_status(&app, &status);
          break;
        }
      }
      maybe_path = path_rx.recv() => {
        if let Some(path) = maybe_path {
          queue.push_back(path);
        }
      }
      _ = poll.tick() => {
        match check_session(&client, &origin, &token, &device_name).await {
          Ok(next_session) => {
            if !next_session.active {
              end_for_cleanup(&app, &status, &token, &next_session);
              break;
            }
          }
          Err(error) => {
            emit_log(&app, &format!("Session check failed: {}", error));
          }
        }
      }
    }
  }
}

async fn process_file(
  app: &AppHandle,
  client: &reqwest::Client,
  origin: &str,
  token: &str,
  device_name: &str,
  root_folder: &Path,
  file_path: &Path,
  cache: &mut LocalCache,
  cache_path: &Path,
  status: &Arc<Mutex<SyncStatus>>,
) -> Result<(), String> {
  let symlink_meta = fs::symlink_metadata(file_path).map_err(|error| {
    format!(
      "Failed reading metadata for {}: {}",
      file_path.display(),
      error
    )
  })?;
  if symlink_meta.file_type().is_symlink() {
    return Err(format!(
      "Symlink entries are blocked for safety: {}",
      file_path.display()
    ));
  }

  let canonical_file_path = fs::canonicalize(file_path).map_err(|error| {
    format!(
      "Failed to resolve canonical path for {}: {}",
      file_path.display(),
      error
    )
  })?;
  if !canonical_file_path.starts_with(root_folder) {
    return Err(format!(
      "Path escapes watched folder: {}",
      canonical_file_path.display()
    ));
  }
  if !canonical_file_path.is_file() {
    return Err("Not a file".into());
  }

  if !is_supported_media(&canonical_file_path) {
    return Err("Unsupported file type. Only images are accepted (no gif or video).".into());
  }

  let relative_path = normalize_relative_path(root_folder, &canonical_file_path)?;
  let stable_snapshot = wait_for_stable_file(
    &canonical_file_path,
    Duration::from_secs(FILE_STABLE_FOR_SECONDS),
    Duration::from_secs(FILE_STABILITY_TIMEOUT_SECONDS),
  )
  .await?;
  if stable_snapshot.size > MAX_FILE_SIZE_BYTES {
    return Err(format!(
      "File too large ({} bytes). Limit is {} bytes: {}",
      stable_snapshot.size, MAX_FILE_SIZE_BYTES, relative_path
    ));
  }

  let expected_content_type = guess_content_type(&canonical_file_path)
    .ok_or_else(|| "Unsupported file type. Only images are accepted.".to_string())?;
  let detected_content_type = detect_content_type_from_magic(&canonical_file_path)?;
  if expected_content_type != detected_content_type {
    return Err(format!(
      "File signature does not match extension for {} (expected {}, detected {})",
      relative_path, expected_content_type, detected_content_type
    ));
  }

  let fingerprint = hash_file(&canonical_file_path)?;
  if cache.uploaded.contains_key(&fingerprint) {
    return Ok(());
  }

  let filename = canonical_file_path
    .file_name()
    .and_then(|name| name.to_str())
    .ok_or_else(|| "Invalid file name".to_string())?;
  let content_type = expected_content_type;

  emit_log(app, &format!("Uploading {}...", relative_path));

  let signed = post_json::<SignedUploadResponse, _>(
    client,
    &format!("{origin}/api/local-sync/signed-upload"),
    &serde_json::json!({
      "token": token,
      "filename": filename,
      "contentType": content_type,
      "deviceName": device_name,
      "fingerprint": fingerprint,
    }),
  )
  .await?;

  if signed.duplicate.unwrap_or(false) {
    let remote_path = signed.path.unwrap_or_default();
    cache.uploaded.insert(
      fingerprint.clone(),
      CachedUpload {
        file_path: canonical_file_path.display().to_string(),
        remote_path,
        uploaded_at: chrono_like_now(),
      },
    );
    save_cache(cache_path, cache)?;
    emit_log(app, &format!("Skipped existing {}", relative_path));
    if let Ok(mut current) = status.lock() {
      current.last_message = Some(format!("Already synced {}", relative_path));
      current.last_error = None;
    }
    emit_status(app, status);
    return Ok(());
  }

  let signed_path = signed
    .path
    .ok_or_else(|| "Signed upload response missing path".to_string())?;
  let signed_url = signed
    .signed_url
    .ok_or_else(|| "Signed upload response missing signed URL".to_string())?;

  let bytes = tokio::fs::read(&canonical_file_path)
    .await
    .map_err(|error| error.to_string())?;
  let after_read_snapshot = file_snapshot(&canonical_file_path)?;
  if after_read_snapshot.size != stable_snapshot.size
    || after_read_snapshot.modified_ms != stable_snapshot.modified_ms
  {
    return Err(format!(
      "File changed while being read (possible partial write): {} (size {} -> {}, modified {} -> {})",
      relative_path,
      stable_snapshot.size,
      after_read_snapshot.size,
      stable_snapshot.modified_ms,
      after_read_snapshot.modified_ms
    ));
  }

  let mut headers = HeaderMap::new();
  headers.insert(
    CONTENT_TYPE,
    HeaderValue::from_str(content_type).map_err(|error| error.to_string())?,
  );
  headers.insert(
    "x-amz-acl",
    HeaderValue::from_static("public-read"),
  );

  client
    .put(&signed_url)
    .headers(headers)
    .body(bytes)
    .send()
    .await
    .map_err(|error| error.to_string())?
    .error_for_status()
    .map_err(|error| error.to_string())?;

  let _: serde_json::Value = post_json(
    client,
    &format!("{origin}/api/local-sync/commit"),
    &serde_json::json!({
      "token": token,
      "path": signed_path,
      "contentType": content_type,
      "originalName": filename,
      "fileSize": after_read_snapshot.size,
      "fingerprint": fingerprint,
      "relativePath": relative_path,
      "deviceName": device_name,
    }),
  )
  .await?;

  cache.uploaded.insert(
    fingerprint.clone(),
    CachedUpload {
      file_path: canonical_file_path.display().to_string(),
      remote_path: signed_path,
      uploaded_at: chrono_like_now(),
    },
  );
  save_cache(cache_path, cache)?;

  if let Ok(mut current) = status.lock() {
    current.files_synced += 1;
    current.last_message = Some(format!("Synced {}", relative_path));
    current.last_error = None;
  }
  emit_status(app, status);
  emit_log(app, &format!("Synced {}", relative_path));

  Ok(())
}

fn create_watcher(path_tx: mpsc::UnboundedSender<PathBuf>) -> notify::Result<RecommendedWatcher> {
  RecommendedWatcher::new(
    move |result: notify::Result<notify::Event>| {
      if let Ok(event) = result {
        for path in event.paths {
          let _ = path_tx.send(path);
        }
      }
    },
    Config::default(),
  )
}

fn enqueue_initial_scan(root: &Path, path_tx: &mpsc::UnboundedSender<PathBuf>) {
  for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
    if entry.file_type().is_file() {
      let _ = path_tx.send(entry.path().to_path_buf());
    }
  }
}

async fn check_session(
  client: &reqwest::Client,
  origin: &str,
  token: &str,
  device_name: &str,
) -> Result<SessionStatusResponse, String> {
  post_json(
    client,
    &format!("{origin}/api/local-sync/session/status"),
    &serde_json::json!({
      "token": token,
      "deviceName": device_name,
    }),
  )
  .await
}

async fn post_json<T, B>(
  client: &reqwest::Client,
  url: &str,
  body: &B,
) -> Result<T, String>
where
  T: for<'de> Deserialize<'de>,
  B: Serialize + ?Sized,
{
  let response = client
    .post(url)
    .json(body)
    .send()
    .await
    .map_err(|error| error.to_string())?;
  let status = response.status();
  let text = response.text().await.map_err(|error| error.to_string())?;

  if !status.is_success() {
    let fallback = format!("Request failed with {}", status);
    let value = serde_json::from_str::<serde_json::Value>(&text)
      .unwrap_or_else(|_| serde_json::json!({ "error": fallback, "body": text }));
    let message = value
      .get("error")
      .and_then(|value| value.as_str())
      .map(|value| value.to_string())
      .unwrap_or(fallback);
    return Err(message);
  }

  serde_json::from_str::<T>(&text).map_err(|error| {
    let preview = text.chars().take(240).collect::<String>();
    if preview.is_empty() {
      format!("Error decoding response body from {}: {}", url, error)
    } else {
      format!(
        "Error decoding response body from {}: {}; body: {}",
        url, error, preview
      )
    }
  })
}

fn load_cache(path: &Path) -> LocalCache {
  fs::read_to_string(path)
    .ok()
    .and_then(|raw| serde_json::from_str(&raw).ok())
    .unwrap_or_default()
}

fn save_cache(path: &Path, cache: &LocalCache) -> Result<(), String> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
  }
  let raw = serde_json::to_string_pretty(cache).map_err(|error| error.to_string())?;
  fs::write(path, raw).map_err(|error| error.to_string())
}

fn cache_file_path(app: &AppHandle, token: &str) -> Result<PathBuf, String> {
  let app_data_dir = app
    .path()
    .app_data_dir()
    .map_err(|error| error.to_string())?;
  let hash = hash_token(token);
  Ok(app_data_dir.join(format!("{hash}.json")))
}

fn hash_token(token: &str) -> String {
  let mut hasher = Sha256::new();
  hasher.update(token.as_bytes());
  format!("{:x}", hasher.finalize())
}

fn normalize_sync_token(value: &str) -> String {
  let trimmed = value.trim();
  if trimmed.is_empty() {
    return String::new();
  }
  if trimmed.starts_with("hgsync_") {
    trimmed.to_string()
  } else {
    format!("hgsync_{trimmed}")
  }
}

fn hash_file(path: &Path) -> Result<String, String> {
  let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
  let mut hasher = Sha256::new();
  let mut buffer = [0_u8; 8 * 1024];

  loop {
    let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
    if count == 0 {
      break;
    }
    hasher.update(&buffer[..count]);
  }

  Ok(format!("{:x}", hasher.finalize()))
}

fn is_supported_media(path: &Path) -> bool {
  path
    .extension()
    .and_then(|value| value.to_str())
    .map(|ext| {
      matches!(
        ext.to_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "webp" | "avif" | "heic" | "heif"
      )
    })
    .unwrap_or(false)
}

fn guess_content_type(path: &Path) -> Option<&'static str> {
  match path
    .extension()
    .and_then(|value| value.to_str())
    .map(|value| value.to_lowercase())
    .as_deref()
  {
    Some("jpg") | Some("jpeg") => Some("image/jpeg"),
    Some("png") => Some("image/png"),
    Some("webp") => Some("image/webp"),
    Some("avif") => Some("image/avif"),
    Some("heic") => Some("image/heic"),
    Some("heif") => Some("image/heif"),
    _ => None,
  }
}

fn detect_content_type_from_magic(path: &Path) -> Result<&'static str, String> {
  let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
  let mut header = [0_u8; 32];
  let count = file.read(&mut header).map_err(|error| error.to_string())?;
  let bytes = &header[..count];

  if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
    return Ok("image/jpeg");
  }
  if bytes.len() >= 8
    && bytes[0] == 0x89
    && bytes[1] == b'P'
    && bytes[2] == b'N'
    && bytes[3] == b'G'
    && bytes[4] == 0x0D
    && bytes[5] == 0x0A
    && bytes[6] == 0x1A
    && bytes[7] == 0x0A
  {
    return Ok("image/png");
  }
  if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
    return Ok("image/webp");
  }
  if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
    let brand = &bytes[8..12];
    if brand == b"avif" || brand == b"avis" {
      return Ok("image/avif");
    }
    if brand == b"heic" || brand == b"heix" || brand == b"hevc" || brand == b"hevx" {
      return Ok("image/heic");
    }
    if brand == b"mif1" || brand == b"msf1" || brand == b"heif" {
      return Ok("image/heif");
    }
  }

  Err(format!(
    "File is not a supported image based on its binary signature: {}",
    path.display()
  ))
}

#[derive(Debug, Clone, Copy)]
struct FileSnapshot {
  size: u64,
  modified_ms: u128,
}

fn file_snapshot(path: &Path) -> Result<FileSnapshot, String> {
  let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
  let modified = metadata.modified().map_err(|error| error.to_string())?;
  let modified_ms = modified
    .duration_since(UNIX_EPOCH)
    .map_err(|error| error.to_string())?
    .as_millis();
  Ok(FileSnapshot {
    size: metadata.len(),
    modified_ms,
  })
}

async fn wait_for_stable_file(
  path: &Path,
  stable_for: Duration,
  timeout: Duration,
) -> Result<FileSnapshot, String> {
  let start = Instant::now();
  let mut last_snapshot = file_snapshot(path)?;
  let mut stable_since = Instant::now();

  loop {
    if stable_since.elapsed() >= stable_for {
      return Ok(last_snapshot);
    }
    if start.elapsed() >= timeout {
      return Err(format!(
        "File did not become stable within {} seconds: {}",
        timeout.as_secs(),
        path.display()
      ));
    }

    sleep(Duration::from_millis(250)).await;
    let current = file_snapshot(path)?;
    if current.size != last_snapshot.size || current.modified_ms != last_snapshot.modified_ms {
      last_snapshot = current;
      stable_since = Instant::now();
    }
  }
}

fn normalize_relative_path(root: &Path, file_path: &Path) -> Result<String, String> {
  let relative = file_path
    .strip_prefix(root)
    .map_err(|_| {
      format!(
        "Cannot compute relative path because {} is outside {}",
        file_path.display(),
        root.display()
      )
    })?;
  Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn normalize_origin(value: &str) -> Result<String, String> {
  if !developer_mode_enabled() {
    return Ok(DEFAULT_ORIGIN.to_string());
  }

  let raw = value.trim().trim_end_matches('/');
  let candidate = if raw.is_empty() { DEFAULT_ORIGIN } else { raw };

  let parsed = Url::parse(candidate).map_err(|_| {
    "Origin must be a full URL like http://localhost:3000 or https://app.heygrats.com"
      .to_string()
  })?;

  let origin = parsed.origin().ascii_serialization();
  if origin == "null" {
    Err(
      "Origin must be a full URL like http://localhost:3000 or https://app.heygrats.com"
        .into(),
    )
  } else {
    Ok(origin)
  }
}

fn developer_mode_enabled() -> bool {
  if cfg!(debug_assertions) {
    return true;
  }

  let value = std::env::var("HEYGRATS_DEVELOPER_MODE").unwrap_or_default();
  matches!(
    value.trim().to_ascii_lowercase().as_str(),
    "1" | "true" | "yes" | "on"
  )
}

fn default_device_name() -> String {
  for key in ["COMPUTERNAME", "HOSTNAME"] {
    if let Ok(value) = std::env::var(key) {
      let trimmed = value.trim();
      if !trimmed.is_empty() {
        return trimmed.to_string();
      }
    }
  }

  "HeyGrats Booth".into()
}

fn display_event_name(event: Option<&EventSummary>) -> String {
  event
    .and_then(|event| event.title.clone().or(event.code.clone()))
    .unwrap_or_else(|| "HeyGrats event".into())
}

fn chrono_like_now() -> String {
  let now = std::time::SystemTime::now();
  let datetime: chrono_like::DateTime = now.into();
  datetime.to_rfc3339()
}

mod chrono_like {
  use std::time::{Duration, SystemTime, UNIX_EPOCH};

  pub struct DateTime {
    inner: SystemTime,
  }

  impl From<SystemTime> for DateTime {
    fn from(value: SystemTime) -> Self {
      Self { inner: value }
    }
  }

  impl DateTime {
    pub fn to_rfc3339(&self) -> String {
      let duration = self
        .inner
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0));
      let secs = duration.as_secs() as i64;
      let tm = time_parts(secs);
      format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        tm.year, tm.month, tm.day, tm.hour, tm.minute, tm.second
      )
    }
  }

  struct Parts {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
  }

  fn time_parts(timestamp: i64) -> Parts {
    let second = timestamp % 60;
    let minute = (timestamp / 60) % 60;
    let hour = (timestamp / 3600) % 24;
    let days = timestamp / 86_400;
    let (year, month, day) = civil_from_days(days);
    Parts {
      year,
      month,
      day,
      hour,
      minute,
      second,
    }
  }

  fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m, d)
  }
}

fn snapshot_status(status: &Arc<Mutex<SyncStatus>>) -> SyncStatus {
  status.lock().map(|value| value.clone()).unwrap_or_default()
}

fn emit_log(app: &AppHandle, message: &str) {
  let _ = app.emit(
    "sync-log",
    LogPayload {
      message: message.to_string(),
    },
  );
}

fn emit_status(app: &AppHandle, status: &Arc<Mutex<SyncStatus>>) {
  let _ = app.emit("sync-status", snapshot_status(status));
}

fn set_error(app: &AppHandle, status: &Arc<Mutex<SyncStatus>>, message: &str) {
  if let Ok(mut current) = status.lock() {
    current.running = false;
    current.last_error = Some(message.to_string());
    current.last_message = Some("Sync stopped due to an error.".into());
  }
  emit_status(app, status);
  emit_log(app, message);
}

fn end_for_cleanup(
  app: &AppHandle,
  status: &Arc<Mutex<SyncStatus>>,
  token: &str,
  session: &SessionStatusResponse,
) {
  let prompt_cleanup = session.prompt_cleanup.unwrap_or(true);
  if let Ok(mut current) = status.lock() {
    current.running = false;
    current.last_error = None;
    current.last_message = Some(format!("Sync ended: {}", session.status));
  }
  emit_status(app, status);
  emit_log(app, &format!("Sync session ended: {}", session.status));

  if prompt_cleanup {
    let _ = app.emit(
      "sync-cleanup",
      CleanupPayload {
        token: token.to_string(),
        message: format!(
          "Sync ended for {}. Remove local cache for this event from this computer?",
          display_event_name(session.event.as_ref())
        ),
      },
    );
  }
}

fn main() {
  tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())
    .manage(SyncRuntime::default())
    .setup(|app| {
      install_resize_lock(app);
      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
      start_sync,
      stop_sync,
      get_sync_status,
      clear_sync_cache
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}

fn install_resize_lock(app: &tauri::App) {
  let Some(window) = app.get_webview_window("main") else {
    return;
  };

  let resize_state = Arc::new(Mutex::new(ResizeLockState::default()));
  let window_handle = window.clone();
  let resize_state_handle = resize_state.clone();

  window.on_window_event(move |event| {
    if let WindowEvent::Resized(size) = event {
      let _ = enforce_aspect_ratio(&window_handle, *size, &resize_state_handle);
    }
  });
}

fn enforce_aspect_ratio(
  window: &WebviewWindow,
  size: PhysicalSize<u32>,
  state: &Arc<Mutex<ResizeLockState>>,
) -> tauri::Result<()> {
  let scale_factor = window.scale_factor()?;
  let logical_size = size.to_logical::<f64>(scale_factor);

  let mut state = match state.lock() {
    Ok(value) => value,
    Err(_) => return Ok(()),
  };

  if state.adjusting {
    state.adjusting = false;
    state.last_width = logical_size.width;
    state.last_height = logical_size.height;
    return Ok(());
  }

  if window.is_maximized()? || window.is_fullscreen()? {
    state.last_width = logical_size.width;
    state.last_height = logical_size.height;
    return Ok(());
  }

  let width_delta = (logical_size.width - state.last_width).abs();
  let height_delta = (logical_size.height - state.last_height).abs();

  let (target_width, target_height) = if width_delta >= height_delta {
    let width = logical_size.width.max(APP_MIN_WIDTH);
    (width, (width / APP_ASPECT_RATIO).round())
  } else {
    let height = logical_size.height.max(APP_MIN_HEIGHT);
    ((height * APP_ASPECT_RATIO).round(), height)
  };

  if (target_width - logical_size.width).abs() < 1.0
    && (target_height - logical_size.height).abs() < 1.0
  {
    state.last_width = logical_size.width;
    state.last_height = logical_size.height;
    return Ok(());
  }

  state.adjusting = true;
  state.last_width = target_width;
  state.last_height = target_height;
  drop(state);

  window.set_size(Size::Logical(LogicalSize::new(
    target_width,
    target_height,
  )))
}
