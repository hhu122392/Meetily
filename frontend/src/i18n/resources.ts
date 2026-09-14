import analyticsEn from "./locales/en/analytics.json";
import commonEn from "./locales/en/common.json";
import importEn from "./locales/en/import.json";
import meetingsEn from "./locales/en/meetings.json";
import mossEn from "./locales/en/moss.json";
import modelsEn from "./locales/en/models.json";
import navigationEn from "./locales/en/navigation.json";
import onboardingEn from "./locales/en/onboarding.json";
import recordingEn from "./locales/en/recording.json";
import settingsEn from "./locales/en/settings.json";
import summaryEn from "./locales/en/summary.json";
import templatesEn from "./locales/en/templates.json";
import transcriptionEn from "./locales/en/transcription.json";
import updatesEn from "./locales/en/updates.json";

import analyticsZhCN from "./locales/zh-CN/analytics.json";
import commonZhCN from "./locales/zh-CN/common.json";
import importZhCN from "./locales/zh-CN/import.json";
import meetingsZhCN from "./locales/zh-CN/meetings.json";
import mossZhCN from "./locales/zh-CN/moss.json";
import modelsZhCN from "./locales/zh-CN/models.json";
import navigationZhCN from "./locales/zh-CN/navigation.json";
import onboardingZhCN from "./locales/zh-CN/onboarding.json";
import recordingZhCN from "./locales/zh-CN/recording.json";
import settingsZhCN from "./locales/zh-CN/settings.json";
import summaryZhCN from "./locales/zh-CN/summary.json";
import templatesZhCN from "./locales/zh-CN/templates.json";
import transcriptionZhCN from "./locales/zh-CN/transcription.json";
import updatesZhCN from "./locales/zh-CN/updates.json";

export const resources = {
  en: {
    analytics: analyticsEn,
    common: commonEn,
    import: importEn,
    meetings: meetingsEn,
    moss: mossEn,
    models: modelsEn,
    navigation: navigationEn,
    onboarding: onboardingEn,
    recording: recordingEn,
    settings: settingsEn,
    summary: summaryEn,
    templates: templatesEn,
    transcription: transcriptionEn,
    updates: updatesEn,
  },
  "zh-CN": {
    analytics: analyticsZhCN,
    common: commonZhCN,
    import: importZhCN,
    meetings: meetingsZhCN,
    moss: mossZhCN,
    models: modelsZhCN,
    navigation: navigationZhCN,
    onboarding: onboardingZhCN,
    recording: recordingZhCN,
    settings: settingsZhCN,
    summary: summaryZhCN,
    templates: templatesZhCN,
    transcription: transcriptionZhCN,
    updates: updatesZhCN,
  },
} as const;
