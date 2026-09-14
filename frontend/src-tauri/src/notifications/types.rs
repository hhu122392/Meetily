use crate::i18n::{translate, SupportedUiLocale};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: Option<String>,
    pub title: String,
    pub body: String,
    pub notification_type: NotificationType,
    pub priority: NotificationPriority,
    pub timeout: NotificationTimeout,
    pub icon: Option<String>,
    pub sound: bool,
    pub actions: Vec<NotificationAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationType {
    RecordingStarted,
    RecordingStopped,
    RecordingPaused,
    RecordingResumed,
    TranscriptionComplete,
    MeetingReminder(u64), // Duration in minutes
    SystemError,
    Test, // For testing notifications
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationPriority {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationTimeout {
    Never,
    Seconds(u64),
    Default,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationAction {
    pub id: String,
    pub title: String,
    pub action_type: NotificationActionType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NotificationActionType {
    Button,
    Reply,
}

impl Notification {
    pub fn new(
        title: impl Into<String>,
        body: impl Into<String>,
        notification_type: NotificationType,
    ) -> Self {
        Self {
            id: None,
            title: title.into(),
            body: body.into(),
            notification_type,
            priority: NotificationPriority::Normal,
            timeout: NotificationTimeout::Default,
            icon: None,
            sound: true,
            actions: vec![],
        }
    }

    pub fn with_priority(mut self, priority: NotificationPriority) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_timeout(mut self, timeout: NotificationTimeout) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_sound(mut self, sound: bool) -> Self {
        self.sound = sound;
        self
    }

    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn add_action(mut self, action: NotificationAction) -> Self {
        self.actions.push(action);
        self
    }
}

impl Default for NotificationPriority {
    fn default() -> Self {
        NotificationPriority::Normal
    }
}

impl Default for NotificationTimeout {
    fn default() -> Self {
        NotificationTimeout::Default
    }
}

// Helper functions for creating common notifications
impl Notification {
    pub fn recording_started(locale: SupportedUiLocale, meeting_name: Option<String>) -> Self {
        let body = match meeting_name {
            Some(name) => translate(
                locale,
                "notification.recordingStartedForMeeting",
                &[("meetingName", &name)],
            ),
            None => translate(locale, "notification.recordingStarted", &[]),
        };

        Notification::new("Meetily", body, NotificationType::RecordingStarted)
            .with_priority(NotificationPriority::High)
            .with_timeout(NotificationTimeout::Seconds(5))
    }

    pub fn recording_stopped(locale: SupportedUiLocale) -> Self {
        Notification::new(
            "Meetily",
            translate(locale, "notification.recordingStoppedAndSaved", &[]),
            NotificationType::RecordingStopped,
        )
        .with_priority(NotificationPriority::Normal)
        .with_timeout(NotificationTimeout::Seconds(3))
    }

    pub fn recording_paused(locale: SupportedUiLocale) -> Self {
        Notification::new(
            "Meetily",
            translate(locale, "notification.recordingPaused", &[]),
            NotificationType::RecordingPaused,
        )
        .with_priority(NotificationPriority::Normal)
        .with_timeout(NotificationTimeout::Seconds(3))
    }

    pub fn recording_resumed(locale: SupportedUiLocale) -> Self {
        Notification::new(
            "Meetily",
            translate(locale, "notification.recordingResumed", &[]),
            NotificationType::RecordingResumed,
        )
        .with_priority(NotificationPriority::Normal)
        .with_timeout(NotificationTimeout::Seconds(3))
    }

    pub fn transcription_complete(locale: SupportedUiLocale, file_path: Option<String>) -> Self {
        // The path is only a signal that persistence occurred. Never expose a
        // local filesystem path in an OS notification.
        let body = match file_path {
            Some(_) => translate(locale, "notification.transcriptionSaved", &[]),
            None => translate(locale, "notification.transcriptionComplete", &[]),
        };

        Notification::new("Meetily", body, NotificationType::TranscriptionComplete)
            .with_priority(NotificationPriority::Normal)
            .with_timeout(NotificationTimeout::Seconds(5))
    }

    pub fn meeting_reminder(
        locale: SupportedUiLocale,
        minutes_until: u64,
        meeting_title: Option<String>,
    ) -> Self {
        let minutes = minutes_until.to_string();
        let body = match meeting_title {
            Some(title) => translate(
                locale,
                "notification.meetingReminderNamed",
                &[("meetingTitle", &title), ("minutes", &minutes)],
            ),
            None => translate(
                locale,
                "notification.meetingReminder",
                &[("minutes", &minutes)],
            ),
        };

        Notification::new(
            "Meetily",
            body,
            NotificationType::MeetingReminder(minutes_until),
        )
        .with_priority(NotificationPriority::High)
        .with_timeout(NotificationTimeout::Seconds(10))
    }

    pub fn system_error(locale: SupportedUiLocale) -> Self {
        Notification::new(
            translate(locale, "notification.systemErrorTitle", &[]),
            translate(locale, "notification.systemErrorBody", &[]),
            NotificationType::SystemError,
        )
        .with_priority(NotificationPriority::Critical)
        .with_timeout(NotificationTimeout::Never)
    }

    pub fn test_notification(locale: SupportedUiLocale) -> Self {
        Notification::new(
            "Meetily",
            translate(locale, "notification.test", &[]),
            NotificationType::Test,
        )
        .with_priority(NotificationPriority::Normal)
        .with_timeout(NotificationTimeout::Seconds(5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_notifications_render_in_both_locales() {
        let english = Notification::recording_started(SupportedUiLocale::En, Some("Design".into()));
        let chinese = Notification::recording_started(SupportedUiLocale::ZhCn, Some("设计".into()));
        assert!(english.body.contains("Design"));
        assert!(chinese.body.contains("设计"));
        assert_ne!(english.body, chinese.body);
    }

    #[test]
    fn every_system_notification_has_distinct_english_and_chinese_copy() {
        let pairs = [
            (
                Notification::recording_stopped(SupportedUiLocale::En),
                Notification::recording_stopped(SupportedUiLocale::ZhCn),
            ),
            (
                Notification::recording_paused(SupportedUiLocale::En),
                Notification::recording_paused(SupportedUiLocale::ZhCn),
            ),
            (
                Notification::recording_resumed(SupportedUiLocale::En),
                Notification::recording_resumed(SupportedUiLocale::ZhCn),
            ),
            (
                Notification::transcription_complete(SupportedUiLocale::En, None),
                Notification::transcription_complete(SupportedUiLocale::ZhCn, None),
            ),
            (
                Notification::meeting_reminder(SupportedUiLocale::En, 5, None),
                Notification::meeting_reminder(SupportedUiLocale::ZhCn, 5, None),
            ),
            (
                Notification::system_error(SupportedUiLocale::En),
                Notification::system_error(SupportedUiLocale::ZhCn),
            ),
            (
                Notification::test_notification(SupportedUiLocale::En),
                Notification::test_notification(SupportedUiLocale::ZhCn),
            ),
        ];

        for (english, chinese) in pairs {
            assert!(!english.body.trim().is_empty());
            assert!(!chinese.body.trim().is_empty());
            assert_ne!(english.body, chinese.body);
        }
    }

    #[test]
    fn notifications_do_not_expose_paths_or_raw_errors() {
        let transcription = Notification::transcription_complete(
            SupportedUiLocale::En,
            Some("C:\\Users\\private\\meeting.wav".into()),
        );
        assert!(!transcription.body.contains("C:\\"));
        let system_error = Notification::system_error(SupportedUiLocale::En);
        assert!(!system_error.body.contains("token="));
    }
}
