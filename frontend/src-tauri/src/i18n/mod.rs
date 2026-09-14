pub mod error;

use crate::tray;
use error::NativeError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{LazyLock, RwLock},
};
use tauri::{AppHandle, Manager, Runtime, State, Wry};

const PREFERENCE_FILE: &str = "ui-locale.json";

static EN_MESSAGES: LazyLock<BTreeMap<String, String>> =
    LazyLock::new(|| parse_messages(include_str!("locales/en.json")));
static ZH_CN_MESSAGES: LazyLock<BTreeMap<String, String>> =
    LazyLock::new(|| parse_messages(include_str!("locales/zh-CN.json")));

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SupportedUiLocale {
    #[serde(rename = "en")]
    En,
    #[serde(rename = "zh-CN")]
    ZhCn,
}

impl SupportedUiLocale {
    pub fn parse(value: &str) -> Result<Self, NativeError> {
        match value {
            "en" => Ok(Self::En),
            "zh-CN" => Ok(Self::ZhCn),
            _ => Err(NativeError::new("I18N_INVALID_LOCALE")),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::ZhCn => "zh-CN",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiLocaleSnapshot {
    pub preference: String,
    pub locale: SupportedUiLocale,
}

impl Default for UiLocaleSnapshot {
    fn default() -> Self {
        Self {
            preference: "system".to_string(),
            locale: SupportedUiLocale::En,
        }
    }
}

pub struct NativeI18nState {
    snapshot: RwLock<UiLocaleSnapshot>,
    preference_path: PathBuf,
}

impl NativeI18nState {
    pub fn load<R: Runtime>(app: &AppHandle<R>) -> Self {
        let preference_path = app
            .path()
            .app_config_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(PREFERENCE_FILE);
        let snapshot = fs::read_to_string(&preference_path)
            .ok()
            .and_then(|value| serde_json::from_str::<UiLocaleSnapshot>(&value).ok())
            .filter(valid_snapshot)
            .unwrap_or_default();

        Self {
            snapshot: RwLock::new(snapshot),
            preference_path,
        }
    }

    #[cfg(test)]
    fn for_test(snapshot: UiLocaleSnapshot, preference_path: PathBuf) -> Self {
        Self {
            snapshot: RwLock::new(snapshot),
            preference_path,
        }
    }

    pub fn snapshot(&self) -> UiLocaleSnapshot {
        self.snapshot
            .read()
            .map(|value| value.clone())
            .unwrap_or_default()
    }

    pub fn locale(&self) -> SupportedUiLocale {
        self.snapshot().locale
    }

    fn replace_and_persist(&self, next: UiLocaleSnapshot) -> Result<bool, NativeError> {
        if !valid_snapshot(&next) {
            return Err(NativeError::new("I18N_INVALID_LOCALE"));
        }

        let mut current = self
            .snapshot
            .write()
            .map_err(|_| NativeError::new("I18N_STATE_UNAVAILABLE"))?;
        if *current == next {
            return Ok(false);
        }

        let serialized = serde_json::to_string_pretty(&next)
            .map_err(|_| NativeError::new("I18N_PERSISTENCE_FAILED"))?;
        if let Some(parent) = self.preference_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                log::error!("Failed to create native locale config directory: {}", error);
                NativeError::new("I18N_PERSISTENCE_FAILED")
            })?;
        }
        fs::write(&self.preference_path, serialized).map_err(|error| {
            log::error!("Failed to persist native locale preference: {}", error);
            NativeError::new("I18N_PERSISTENCE_FAILED")
        })?;
        *current = next;
        Ok(true)
    }
}

pub fn current_locale<R: Runtime>(app: &AppHandle<R>) -> SupportedUiLocale {
    app.try_state::<NativeI18nState>()
        .map(|state| state.locale())
        .unwrap_or(SupportedUiLocale::En)
}

pub fn translate(locale: SupportedUiLocale, key: &str, params: &[(&str, &str)]) -> String {
    let primary = match locale {
        SupportedUiLocale::En => &*EN_MESSAGES,
        SupportedUiLocale::ZhCn => &*ZH_CN_MESSAGES,
    };
    let template = primary
        .get(key)
        .or_else(|| EN_MESSAGES.get(key))
        .cloned()
        .unwrap_or_else(|| key.to_string());
    params.iter().fold(template, |message, (name, value)| {
        message.replace(&format!("{{{{{}}}}}", name), value)
    })
}

pub fn translate_for_app<R: Runtime>(
    app: &AppHandle<R>,
    key: &str,
    params: &[(&str, &str)],
) -> String {
    translate(current_locale(app), key, params)
}

#[tauri::command]
pub fn get_ui_locale(state: State<'_, NativeI18nState>) -> UiLocaleSnapshot {
    state.snapshot()
}

#[tauri::command]
pub fn set_ui_locale(
    app: AppHandle<Wry>,
    state: State<'_, NativeI18nState>,
    preference: String,
    locale: String,
) -> Result<UiLocaleSnapshot, NativeError> {
    let parsed_locale = SupportedUiLocale::parse(&locale)?;
    let next = UiLocaleSnapshot {
        preference,
        locale: parsed_locale,
    };
    let changed = state.replace_and_persist(next.clone())?;
    if changed {
        tray::refresh_tray_for_locale(&app);
        log::info!("Native UI locale changed to {}", parsed_locale.as_str());
    }
    Ok(next)
}

fn parse_messages(source: &str) -> BTreeMap<String, String> {
    serde_json::from_str::<BTreeMap<String, Value>>(source)
        .expect("native locale resource must be valid JSON")
        .into_iter()
        .map(|(key, value)| {
            let message = value
                .as_str()
                .unwrap_or_else(|| panic!("native locale value must be a string: {key}"));
            (key, message.to_string())
        })
        .collect()
}

fn valid_snapshot(snapshot: &UiLocaleSnapshot) -> bool {
    matches!(snapshot.preference.as_str(), "system" | "en" | "zh-CN")
        && match snapshot.preference.as_str() {
            "en" => snapshot.locale == SupportedUiLocale::En,
            "zh-CN" => snapshot.locale == SupportedUiLocale::ZhCn,
            "system" => true,
            _ => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_resources_have_identical_keys_and_string_values() {
        assert_eq!(
            EN_MESSAGES.keys().collect::<Vec<_>>(),
            ZH_CN_MESSAGES.keys().collect::<Vec<_>>()
        );
        assert!(EN_MESSAGES.values().all(|value| !value.trim().is_empty()));
        assert!(ZH_CN_MESSAGES
            .values()
            .all(|value| !value.trim().is_empty()));
    }

    #[test]
    fn translator_interpolates_and_falls_back_safely() {
        assert_eq!(
            translate(
                SupportedUiLocale::ZhCn,
                "notification.recordingStartedForMeeting",
                &[("meetingName", "设计评审")],
            ),
            "已开始录制会议：设计评审"
        );
        assert_eq!(
            translate(SupportedUiLocale::ZhCn, "missing.key", &[]),
            "missing.key"
        );
    }

    #[test]
    fn persistence_round_trip_and_idempotence() {
        let path = std::env::temp_dir().join(format!("meetily-i18n-{}.json", uuid::Uuid::new_v4()));
        let state = NativeI18nState::for_test(UiLocaleSnapshot::default(), path.clone());
        let next = UiLocaleSnapshot {
            preference: "zh-CN".to_string(),
            locale: SupportedUiLocale::ZhCn,
        };
        assert_eq!(state.replace_and_persist(next.clone()), Ok(true));
        assert_eq!(state.replace_and_persist(next.clone()), Ok(false));
        let persisted: UiLocaleSnapshot =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(persisted, next);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn explicit_preferences_must_match_resolved_locale() {
        assert!(!valid_snapshot(&UiLocaleSnapshot {
            preference: "en".to_string(),
            locale: SupportedUiLocale::ZhCn,
        }));
        assert!(valid_snapshot(&UiLocaleSnapshot {
            preference: "system".to_string(),
            locale: SupportedUiLocale::ZhCn,
        }));
    }
}
