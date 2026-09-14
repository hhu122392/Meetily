import React from 'react';
import { useTranslation } from 'react-i18next';

interface ConfirmationModalProps {
  onConfirm: () => void;
  onCancel: () => void;
  text: string;
  isOpen: boolean;
  /** 可选覆盖：默认走 `common` 语言包（P1-13） */
  title?: string;
  confirmLabel?: string;
  cancelLabel?: string;
}

export function ConfirmationModal({
  onConfirm,
  onCancel,
  text,
  isOpen,
  title,
  confirmLabel,
  cancelLabel,
}: ConfirmationModalProps) {
  const { t } = useTranslation('common');
  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center z-50">
      <div className="bg-white rounded-lg p-6 max-w-md w-full mx-4">
        <h2 className="text-xl font-semibold mb-4">{title ?? t('labels.confirmDelete')}</h2>
        <p className="text-gray-600 mb-6">{text}</p>
        <div className="flex justify-end space-x-4">
          <button
            onClick={onCancel}
            className="px-4 py-2 text-gray-600 hover:bg-gray-100 rounded-md transition-colors"
          >
            {cancelLabel ?? t('actions.cancel')}
          </button>
          <button
            onClick={onConfirm}
            className="px-4 py-2 bg-red-600 text-white hover:bg-red-700 rounded-md transition-colors"
          >
            {confirmLabel ?? t('actions.delete')}
          </button>
        </div>
      </div>
    </div>
  );
}
