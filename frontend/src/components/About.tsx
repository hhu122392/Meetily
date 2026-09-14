import React, { useState, useEffect } from "react";
import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import Image from 'next/image';
import AnalyticsConsentSwitch from "./AnalyticsConsentSwitch";
import { UpdateDialog } from "./UpdateDialog";
import { updateService, UpdateInfo } from '@/services/updateService';
import { Button } from './ui/button';
import { Loader2, CheckCircle2 } from 'lucide-react';
import { toast } from 'sonner';
import { useTranslation } from 'react-i18next';


export function About() {
    const { t } = useTranslation(['common', 'updates']);
    const [currentVersion, setCurrentVersion] = useState<string>('0.4.0');
    const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
    const [isChecking, setIsChecking] = useState(false);
    const [showUpdateDialog, setShowUpdateDialog] = useState(false);

    useEffect(() => {
        // Get current version on mount
        getVersion().then(setCurrentVersion).catch(console.error);
    }, []);

    const handleContactClick = async () => {
        try {
            await invoke('open_external_url', { url: 'https://github.com/hhu122392/Meetily/issues' });
        } catch (error) {
            console.error('Failed to open link:', error);
            toast.error(t('common:errors.failedToOpenExternalLink'));
        }
    };

    const handleCheckForUpdates = async () => {
        setIsChecking(true);
        try {
            const info = await updateService.checkForUpdates(true);
            setUpdateInfo(info);
            if (info.manual) return;
            if (info.available) {
                setShowUpdateDialog(true);
            } else {
                toast.success(t('updates:messages.runningLatestVersion'));
            }
        } catch (error) {
            console.error('Failed to check for updates:', error);
            toast.error(t('updates:errors.checkFailed'));
        } finally {
            setIsChecking(false);
        }
    };

    return (
        <div className="p-4 space-y-4 h-[80vh] overflow-y-auto">
            {/* Compact Header */}
            <div className="text-center">
                <div className="mb-3">
                    <Image
                        src="icon_128x128.png"
                        alt={t('common:labels.aboutMeetily')}
                        width={64}
                        height={64}
                        className="mx-auto"
                    />
                </div>
                <span className="text-sm text-gray-500"> v{currentVersion}</span>
                <p className="text-medium text-gray-600 mt-1">
                    {t('common:descriptions.aboutTagline')}
                </p>
                <div className="mt-3">
                    <Button
                        onClick={handleCheckForUpdates}
                        disabled={isChecking}
                        variant="outline"
                        size="sm"
                        className="text-xs"
                    >
                        {isChecking ? (
                            <>
                                <Loader2 className="h-3 w-3 mr-2 animate-spin" />
                                {t('updates:actions.checking')}
                            </>
                        ) : (
                            <>
                                <CheckCircle2 className="h-3 w-3 mr-2" />
                                {t('updates:actions.viewReleases')}
                            </>
                        )}
                    </Button>
                    {updateInfo?.available && (
                        <div className="mt-2 text-xs text-blue-600">
                            {t('updates:messages.updateAvailableValue', { version: updateInfo.version })}
                        </div>
                    )}
                </div>
            </div>

            {/* Features Grid - Compact */}
            <div className="space-y-3">
                <h2 className="text-base font-semibold text-gray-800">{t('common:labels.whatMakesMeetilyDifferent')}</h2>
                <div className="grid grid-cols-2 gap-2">
                    <div className="bg-gray-50 rounded p-3 hover:bg-gray-100 transition-colors">
                        <h3 className="font-bold text-sm text-gray-900 mb-1">{t('common:labels.privacyFirst')}</h3>
                        <p className="text-xs text-gray-600 leading-relaxed">{t('common:descriptions.privacyFirst')}</p>
                    </div>
                    <div className="bg-gray-50 rounded p-3 hover:bg-gray-100 transition-colors">
                        <h3 className="font-bold text-sm text-gray-900 mb-1">{t('common:labels.useAnyModel')}</h3>
                        <p className="text-xs text-gray-600 leading-relaxed">{t('common:descriptions.useAnyModel')}</p>
                    </div>
                    <div className="bg-gray-50 rounded p-3 hover:bg-gray-100 transition-colors">
                        <h3 className="font-bold text-sm text-gray-900 mb-1">{t('common:labels.costSmart')}</h3>
                        <p className="text-xs text-gray-600 leading-relaxed">{t('common:descriptions.costSmart')}</p>
                    </div>
                    <div className="bg-gray-50 rounded p-3 hover:bg-gray-100 transition-colors">
                        <h3 className="font-bold text-sm text-gray-900 mb-1">{t('common:labels.worksEverywhere')}</h3>
                        <p className="text-xs text-gray-600 leading-relaxed">{t('common:descriptions.worksEverywhere')}</p>
                    </div>
                </div>
            </div>

            {/* Coming Soon - Compact */}
            <div className="bg-blue-50 rounded p-3">
                <p className="text-s text-blue-800">
                    <span className="font-bold">{t('common:labels.comingSoon')}</span>{' '}{t('common:descriptions.comingSoon')}
                </p>
            </div>

            {/* CTA Section - Compact */}
            <div className="text-center space-y-2">
                <h3 className="text-medium font-semibold text-gray-800">{t('common:labels.readyToPushYourBusinessFurther')}</h3>
                <p className="text-s text-gray-600">
                    {t('common:descriptions.contactTeam')}
                </p>
                <button
                    onClick={handleContactClick}
                    className="inline-flex items-center px-4 py-2 bg-blue-600 hover:bg-blue-700 text-white text-sm font-medium rounded transition-colors duration-200 shadow-sm hover:shadow-md"
                >
                    {t('common:actions.chatWithTeam')}
                </button>
            </div>

            {/* Footer - Compact */}
            <div className="pt-2 border-t border-gray-200 text-center">
                <p className="text-xs text-gray-400">
                    {t('common:labels.builtByZackriyaSolutions')}
                </p>
            </div>
            <AnalyticsConsentSwitch />

            {/* Update Dialog */}
            <UpdateDialog
                open={showUpdateDialog}
                onOpenChange={setShowUpdateDialog}
                updateInfo={updateInfo}
            />
        </div>

    )
}
