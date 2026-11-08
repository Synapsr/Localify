//! Remote machines (`docs/remote-machines.md`, "Desktop app"). One-shot
//! commands call `DesktopApi`; a streaming management command reaches the
//! window as `remote-stream` events, one per line, until the window closes
//! the stream or the daemon ends it.

use std::collections::HashMap;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use localify::desktop::dto::{
    RemoteMachineRequest, RemotePairAnswerRequest, RemoteSettingsRequest, RemoteStreamRequest,
};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, Runtime, async_runtime};

use crate::commands::AppState;

pub const REMOTE_STREAM_EVENT: &str = "remote-stream";

/// The streams the window has open, by id, each with the handle that
/// closes it.
#[derive(Default)]
pub struct RemoteStreams {
    next: AtomicU64,
    open: Mutex<HashMap<u64, UnixStream>>,
}

impl RemoteStreams {
    fn open(&self) -> MutexGuard<'_, HashMap<u64, UnixStream>> {
        self.open
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn close(&self, id: u64) {
        if let Some(stream) = self.open().remove(&id) {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

/// One line of a stream, or its end: `done` with the error that ended it,
/// if any.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteStreamPayload {
    stream_id: u64,
    data: Option<Value>,
    done: bool,
    error: Option<String>,
}

fn map_error<E: std::fmt::Display>(error: E) -> String {
    error.to_string()
}

#[tauri::command]
pub async fn get_remote_status(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.remote_status().map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn get_remote_machines(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.remote_machines().map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn change_remote_settings(
    state: tauri::State<'_, AppState>,
    request: RemoteSettingsRequest,
) -> Result<Value, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.change_remote_settings(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn answer_remote_pairing(
    state: tauri::State<'_, AppState>,
    request: RemotePairAnswerRequest,
) -> Result<(), String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.answer_remote_pairing(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn unpair_remote_machine(
    state: tauri::State<'_, AppState>,
    request: RemoteMachineRequest,
) -> Result<bool, String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.unpair_remote_machine(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

#[tauri::command]
pub async fn remove_remote_machine(
    state: tauri::State<'_, AppState>,
    request: RemoteMachineRequest,
) -> Result<(), String> {
    let api = state.api.clone();
    async_runtime::spawn_blocking(move || api.remove_remote_machine(request).map_err(map_error))
        .await
        .map_err(map_error)?
}

/// Opens a streaming command and returns its id; its lines come as
/// `remote-stream` events.
#[tauri::command]
pub async fn open_remote_stream<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    streams: tauri::State<'_, RemoteStreams>,
    request: RemoteStreamRequest,
) -> Result<u64, String> {
    let api = state.api.clone();
    let mut stream = async_runtime::spawn_blocking(move || api.open_remote_stream(request))
        .await
        .map_err(map_error)?
        .map_err(map_error)?;
    let closer = stream.closer().map_err(map_error)?;
    let id = streams.next.fetch_add(1, Ordering::SeqCst) + 1;
    streams.open().insert(id, closer);
    std::thread::spawn(move || {
        let error = loop {
            match stream.next_answer() {
                Ok(Some(data)) => {
                    let line = RemoteStreamPayload {
                        stream_id: id,
                        data: Some(data),
                        done: false,
                        error: None,
                    };
                    let _ = app.emit(REMOTE_STREAM_EVENT, line);
                }
                Ok(None) => break None,
                Err(error) => break Some(error.to_string()),
            }
        };
        // Closed by the window: nothing to tell it.
        let open = app.state::<RemoteStreams>().open().remove(&id).is_some();
        if open {
            let end = RemoteStreamPayload {
                stream_id: id,
                data: None,
                done: true,
                error,
            };
            let _ = app.emit(REMOTE_STREAM_EVENT, end);
        }
    });
    Ok(id)
}

#[tauri::command]
pub fn close_remote_stream(streams: tauri::State<'_, RemoteStreams>, stream_id: u64) {
    streams.close(stream_id);
}
