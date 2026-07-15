use std::{
    fs,
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::diagnostics;

const PROFILES_FILE: &str = "profiles.json";
const MAX_PROFILE_NAME_LENGTH: usize = 80;
const MAX_PROFILE_PROMPT_LENGTH: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    id: u64,
    name: String,
    prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfilesSnapshot {
    profiles: Vec<Profile>,
    active_profile_id: Option<u64>,
    next_id: u64,
}

impl Default for ProfilesSnapshot {
    fn default() -> Self {
        Self {
            profiles: vec![Profile {
                id: 1,
                name: "Default profile".to_string(),
                prompt: String::new(),
            }],
            active_profile_id: None,
            next_id: 2,
        }
    }
}

#[derive(Debug, Default)]
pub struct ProfileStore {
    snapshot: Mutex<ProfilesSnapshot>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProfileRequest {
    id: u64,
    name: String,
    prompt: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileIdRequest {
    id: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCommandError {
    code: &'static str,
    message: String,
}

type CommandResult<T> = Result<T, ProfileCommandError>;

pub fn setup(app: &AppHandle) {
    let Ok(Some(snapshot)) = load_profiles(app) else {
        return;
    };
    if let Ok(mut stored) = app.state::<ProfileStore>().snapshot.lock() {
        *stored = normalize_snapshot(snapshot);
    }
}

#[tauri::command]
pub fn profiles_get_state(state: State<'_, ProfileStore>) -> CommandResult<ProfilesSnapshot> {
    Ok(state.snapshot()?.clone())
}

#[tauri::command]
pub fn profiles_add(
    app: AppHandle,
    state: State<'_, ProfileStore>,
) -> CommandResult<ProfilesSnapshot> {
    let mut snapshot = state.snapshot()?.clone();
    let id = snapshot.next_id;
    snapshot.next_id = snapshot.next_id.saturating_add(1).max(id.saturating_add(1));
    snapshot.profiles.push(Profile {
        id,
        name: next_profile_name(&snapshot.profiles),
        prompt: String::new(),
    });
    save_profiles(&app, &snapshot).map_err(ProfileCommandError::storage)?;
    *state.snapshot()? = snapshot.clone();
    diagnostics::record(&app, "INFO", "profiles", &format!("added profile id={id}"));
    Ok(snapshot)
}

#[tauri::command]
pub fn profiles_save(
    app: AppHandle,
    state: State<'_, ProfileStore>,
    request: SaveProfileRequest,
) -> CommandResult<ProfilesSnapshot> {
    let name = validate_name(&request.name)?;
    let prompt = validate_prompt(&request.prompt)?;
    let mut snapshot = state.snapshot()?.clone();
    let profile = snapshot
        .profiles
        .iter_mut()
        .find(|profile| profile.id == request.id)
        .ok_or_else(ProfileCommandError::not_found)?;
    profile.name = name;
    profile.prompt = prompt;
    save_profiles(&app, &snapshot).map_err(ProfileCommandError::storage)?;
    *state.snapshot()? = snapshot.clone();
    diagnostics::record(
        &app,
        "INFO",
        "profiles",
        &format!("saved profile id={}", request.id),
    );
    Ok(snapshot)
}

#[tauri::command]
pub fn profiles_delete(
    app: AppHandle,
    state: State<'_, ProfileStore>,
    request: ProfileIdRequest,
) -> CommandResult<ProfilesSnapshot> {
    let mut snapshot = state.snapshot()?.clone();
    let previous_len = snapshot.profiles.len();
    snapshot.profiles.retain(|profile| profile.id != request.id);
    if snapshot.profiles.len() == previous_len {
        return Err(ProfileCommandError::not_found());
    }
    if snapshot.active_profile_id == Some(request.id) {
        snapshot.active_profile_id = None;
    }
    save_profiles(&app, &snapshot).map_err(ProfileCommandError::storage)?;
    *state.snapshot()? = snapshot.clone();
    diagnostics::record(
        &app,
        "INFO",
        "profiles",
        &format!("deleted profile id={}", request.id),
    );
    Ok(snapshot)
}

#[tauri::command]
pub fn profiles_activate(
    app: AppHandle,
    state: State<'_, ProfileStore>,
    request: ProfileIdRequest,
) -> CommandResult<ProfilesSnapshot> {
    let mut snapshot = state.snapshot()?.clone();
    if snapshot
        .profiles
        .iter()
        .all(|profile| profile.id != request.id)
    {
        return Err(ProfileCommandError::not_found());
    }
    snapshot.active_profile_id = Some(request.id);
    save_profiles(&app, &snapshot).map_err(ProfileCommandError::storage)?;
    *state.snapshot()? = snapshot.clone();

    diagnostics::record(
        &app,
        "INFO",
        "profiles",
        &format!("activated profile id={}", request.id),
    );
    Ok(snapshot)
}

pub fn active_prompt(app: &AppHandle) -> Result<Option<String>, String> {
    let store = app.state::<ProfileStore>();
    let snapshot = store
        .snapshot
        .lock()
        .map_err(|_| "Profile state is unavailable.".to_string())?;
    Ok(snapshot.active_profile_id.and_then(|active_id| {
        snapshot
            .profiles
            .iter()
            .find(|profile| profile.id == active_id)
            .map(|profile| profile.prompt.clone())
    }))
}

impl ProfileStore {
    fn snapshot(&self) -> CommandResult<MutexGuard<'_, ProfilesSnapshot>> {
        self.snapshot.lock().map_err(|_| ProfileCommandError {
            code: "state_unavailable",
            message: "Profile state is unavailable.".to_string(),
        })
    }
}

impl ProfileCommandError {
    fn validation(message: impl Into<String>) -> Self {
        Self {
            code: "validation_error",
            message: message.into(),
        }
    }

    fn not_found() -> Self {
        Self {
            code: "profile_not_found",
            message: "The selected profile no longer exists.".to_string(),
        }
    }

    fn storage(message: String) -> Self {
        Self {
            code: "storage_error",
            message,
        }
    }
}

fn validate_name(name: &str) -> CommandResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ProfileCommandError::validation(
            "Profile name cannot be empty.",
        ));
    }
    if name.chars().count() > MAX_PROFILE_NAME_LENGTH {
        return Err(ProfileCommandError::validation(format!(
            "Profile name cannot exceed {MAX_PROFILE_NAME_LENGTH} characters."
        )));
    }
    Ok(name.to_string())
}

fn validate_prompt(prompt: &str) -> CommandResult<String> {
    if prompt.chars().count() > MAX_PROFILE_PROMPT_LENGTH {
        return Err(ProfileCommandError::validation(format!(
            "Profile prompt cannot exceed {MAX_PROFILE_PROMPT_LENGTH} characters."
        )));
    }
    Ok(prompt.trim().to_string())
}

fn next_profile_name(profiles: &[Profile]) -> String {
    let mut number = profiles.len() + 1;
    loop {
        let candidate = format!("Profile {number}");
        if profiles.iter().all(|profile| profile.name != candidate) {
            return candidate;
        }
        number += 1;
    }
}

fn normalize_snapshot(mut snapshot: ProfilesSnapshot) -> ProfilesSnapshot {
    let max_id = snapshot
        .profiles
        .iter()
        .map(|profile| profile.id)
        .max()
        .unwrap_or(0);
    snapshot.next_id = snapshot.next_id.max(max_id.saturating_add(1)).max(1);
    if snapshot.active_profile_id.is_some_and(|active_id| {
        snapshot
            .profiles
            .iter()
            .all(|profile| profile.id != active_id)
    }) {
        snapshot.active_profile_id = None;
    }
    snapshot
}

fn profiles_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|directory| directory.join(PROFILES_FILE))
        .map_err(|error| format!("Failed to resolve profile storage directory: {error}"))
}

fn load_profiles(app: &AppHandle) -> Result<Option<ProfilesSnapshot>, String> {
    let path = profiles_path(app)?;
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path)
        .map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
    serde_json::from_str(&contents)
        .map(Some)
        .map_err(|error| format!("Saved profiles are invalid: {error}"))
}

fn save_profiles(app: &AppHandle, snapshot: &ProfilesSnapshot) -> Result<(), String> {
    let path = profiles_path(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Profile storage path has no parent directory.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create {}: {error}", parent.display()))?;
    let contents = serde_json::to_string_pretty(snapshot)
        .map_err(|error| format!("Failed to serialize profiles: {error}"))?;
    fs::write(&path, contents)
        .map_err(|error| format!("Failed to save {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{next_profile_name, normalize_snapshot, validate_name, Profile, ProfilesSnapshot};

    #[test]
    fn profile_names_are_required_and_trimmed() {
        assert!(validate_name("   ").is_err());
        assert_eq!(validate_name("  Interview  ").unwrap(), "Interview");
    }

    #[test]
    fn new_profile_names_skip_existing_names() {
        let profiles = vec![
            Profile {
                id: 1,
                name: "Default profile".to_string(),
                prompt: String::new(),
            },
            Profile {
                id: 2,
                name: "Profile 3".to_string(),
                prompt: String::new(),
            },
        ];
        assert_eq!(next_profile_name(&profiles), "Profile 4");
    }

    #[test]
    fn stale_active_profile_is_cleared_when_loading() {
        let snapshot = normalize_snapshot(ProfilesSnapshot {
            profiles: vec![Profile {
                id: 7,
                name: "Saved".to_string(),
                prompt: "Prompt".to_string(),
            }],
            active_profile_id: Some(99),
            next_id: 1,
        });
        assert_eq!(snapshot.active_profile_id, None);
        assert_eq!(snapshot.next_id, 8);
    }
}
