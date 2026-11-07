'use client';
import Link from 'next/link';
import { TemplateLibrary } from '@/components/templates/TemplateLibrary';

export default function TemplatesPage() {
  return <main className="h-screen overflow-y-auto bg-gray-50 p-4 sm:p-8"><div className="mx-auto max-w-6xl"><Link className="mb-6 inline-block text-sm text-blue-700" href="/settings">← 返回设置</Link><TemplateLibrary/></div></main>;
}
