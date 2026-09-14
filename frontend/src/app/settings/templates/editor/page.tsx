import { Suspense } from 'react';
import { TemplateEditorPage } from '@/components/templates/TemplateEditorPage';

export default function SummaryTemplateEditorRoute() {
  return (
    <Suspense fallback={null}>
      <TemplateEditorPage />
    </Suspense>
  );
}
