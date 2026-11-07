'use client';

import { useEffect, useRef, useState } from 'react';
import { Plus, Copy, Download, Upload, Star, Trash2, ArrowUp, ArrowDown } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogTitle, DialogDescription } from '@/components/ui/dialog';
import { MAX_TEMPLATE_BYTES, newSection, newTemplate, templateLibrary, TEMPLATES_CHANGED, validateTemplateDraft, type TemplateDocument, type TemplateEntry, type TemplateLibrary as Library, type TemplateSection } from '@/lib/summary-template';

const field = 'w-full min-w-0 rounded-lg border border-gray-200 bg-white px-3 py-2 text-sm outline-none focus:border-blue-400 focus:ring-2 focus:ring-blue-100 disabled:bg-gray-50';
type Editor = { doc: TemplateDocument; original: string; expectedVersion: number | null; readonly: boolean };

export function TemplateLibrary() {
  const [library, setLibrary] = useState<Library | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [search, setSearch] = useState('');
  const [editor, setEditor] = useState<Editor | null>(null);
  const [discard, setDiscard] = useState(false);
  const [deleting, setDeleting] = useState<TemplateEntry | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const mounted = useRef(true);
  const busyRef = useRef(false);
  const loadVersion = useRef(0);
  useEffect(() => { mounted.current = true; void refresh().catch(e => {if(mounted.current)setError(String(e));}); return () => { mounted.current = false; }; }, []);

  async function refresh() {
    const version = ++loadVersion.current;
    const result = await templateLibrary.list();
    if (mounted.current && version === loadVersion.current) setLibrary(result);
  }
  async function run(action: () => Promise<void>) {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true); setError(''); setNotice('');
    try { await action(); } catch (e) { if (mounted.current) setError(String(e)); }
    finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  }
  async function changed(message: string) {
    window.dispatchEvent(new Event(TEMPLATES_CHANGED));
    await refresh();
    if (mounted.current) setNotice(message);
  }
  function edit(doc: TemplateDocument, readonly: boolean, expectedVersion: number | null) {
    if (!mounted.current) return;
    setEditor({ doc, readonly, expectedVersion, original: JSON.stringify(doc) }); setDiscard(false);
  }
  function patch(patch: Partial<TemplateDocument>) {
    setEditor(current => current ? { ...current, doc: { ...current.doc, ...patch } } : current);
  }
  function patchSection(index: number, patchValue: Partial<TemplateSection>) {
    if (editor) patch({ sections: editor.doc.sections.map((s, i) => i === index ? { ...s, ...patchValue } : s) });
  }
  function move(index: number, offset: number) {
    if (!editor) return;
    const sections = [...editor.doc.sections];
    [sections[index], sections[index + offset]] = [sections[index + offset], sections[index]];
    patch({ sections });
  }
  function closeEditor() {
    if (busy) return;
    if (editor && JSON.stringify(editor.doc) !== editor.original) setDiscard(true);
    else { setEditor(null); setError(''); }
  }
  async function exportTemplate(entry: TemplateEntry) {
    const path = await templateLibrary.export(entry.id);
    if (mounted.current) setNotice(path ? `模板已导出：${path}` : '已取消导出');
  }
  const filtered = library?.templates.filter(item => `${item.name} ${item.description}`.toLowerCase().includes(search.toLowerCase())) ?? [];

  return <section className="space-y-5">
    <div className="flex flex-wrap items-start justify-between gap-3">
      <div><h2 className="text-xl font-semibold">纪要模板库</h2><p className="mt-1 text-sm text-gray-600">管理当前公司的模板。内置模板可复制后修改，每场会议也可以单独选择。</p></div>
      <div className="flex gap-2">
        <Button variant="outline" disabled={busy || !library} onClick={() => fileRef.current?.click()}><Upload size={16}/>导入 JSON</Button>
        <Button disabled={busy || !library} onClick={() => {setError(''); edit(newTemplate(), false, null);}}><Plus size={16}/>新建模板</Button>
      </div>
    </div>
    <input ref={fileRef} type="file" accept=".json,application/json" className="hidden" aria-label="导入模板 JSON" onChange={event => {
      const file = event.target.files?.[0]; event.target.value = '';
      if (file) void run(async () => {
        if (file.size > MAX_TEMPLATE_BYTES) throw Error('模板文件不能超过 800KB');
        await templateLibrary.import(await file.text()); await changed('模板已导入为新副本，原模板未被覆盖');
      });
    }}/>
    {error && !editor && !deleting && <p role="alert" className="rounded-md bg-red-50 p-3 text-red-700">{error}</p>}
    {notice && <p role="status" className="text-sm text-green-700">{notice}</p>}
    {library?.warnings.map((warning, i) => <p key={i} role="alert" className="rounded-md bg-amber-50 p-3 text-sm text-amber-900">{warning}</p>)}
    <div className="flex gap-2"><input className={field} placeholder="搜索模板名称或说明" aria-label="搜索模板" value={search} onChange={e => setSearch(e.target.value)}/><Button variant="outline" disabled={busy} onClick={() => void run(refresh)}>刷新</Button></div>
    {!library && <p className="text-gray-500">{error ? '模板库加载失败，请重试。' : '正在加载模板…'}</p>}
    {library && !filtered.length && <p className="py-10 text-center text-gray-500">没有匹配的模板</p>}
    <div className="grid gap-4 lg:grid-cols-2">{filtered.map(entry => <article key={entry.id} className="flex min-w-0 flex-col gap-3 rounded-xl border border-gray-200 bg-white p-5 shadow-sm">
      <div className="flex flex-wrap items-center gap-2"><h3 className="font-semibold">{entry.name}</h3>{library?.defaultTemplateId === entry.id && <span className="rounded-full bg-blue-50 px-2 py-0.5 text-xs text-blue-700">默认</span>}</div>
      <p className="flex-1 text-sm text-gray-600">{entry.description}</p>
      <p className="text-xs text-gray-500">{entry.readonly ? '内置模板' : '自定义模板'} · {entry.locale || '未指定语言'} · 版本 {entry.version}</p>
      <div className="flex flex-wrap gap-2">
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void run(async () => edit(await templateLibrary.read(entry.id), entry.readonly, entry.version))}>{entry.readonly ? '查看' : '编辑'}</Button>
        <Button size="sm" variant="outline" disabled={busy} onClick={() => void run(async () => {const doc = await templateLibrary.duplicate(entry.id); await changed('模板副本已创建'); edit(doc, false, doc.version);})}><Copy size={14}/>复制</Button>
        <Button size="sm" variant="outline" disabled={busy || library?.defaultTemplateId === entry.id} onClick={() => void run(async () => {await templateLibrary.setDefault(entry.id); await changed('默认模板已保存，将用于尚未设置模板的会议');})}><Star size={14}/>设为默认</Button>
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => void run(() => exportTemplate(entry))}><Download size={14}/>导出</Button>
        {!entry.readonly && <Button size="sm" variant="ghost" disabled={busy} onClick={() => {setError(''); setDeleting(entry);}}><Trash2 size={14}/>删除</Button>}
      </div>
    </article>)}</div>

    <Dialog open={!!editor} onOpenChange={open => {if (!open) closeEditor();}}>
      <DialogContent className="flex w-[calc(100vw-2rem)] max-w-3xl max-h-[90vh] flex-col overflow-hidden rounded-2xl">
        <DialogTitle>{editor?.readonly ? '查看内置模板' : editor?.expectedVersion === null ? '新建纪要模板' : '编辑纪要模板'}</DialogTitle>
        <DialogDescription>{editor?.readonly ? '内置模板保持不变，关闭后可在模板库中复制编辑。' : '按章节设置纪要结构与生成要求，保存后在会议中选择使用。'}</DialogDescription>
        {editor && <>
          <div className="min-h-0 flex-1 overflow-y-auto px-1">
          {error && <p role="alert" className="text-sm text-red-700">{error}</p>}
          <fieldset disabled={editor.readonly || busy} className="space-y-4">
            <label className="block space-y-1"><span>模板名称</span><input className={field} value={editor.doc.name} maxLength={120} onChange={e => patch({name:e.target.value})}/></label>
            <label className="block space-y-1"><span>模板说明</span><textarea className={field} rows={2} value={editor.doc.description} maxLength={1000} onChange={e => patch({description:e.target.value})}/></label>
            <label className="block space-y-1"><span>模板内容语言</span><input className={field} placeholder="zh-CN、en 或留空" value={editor.doc.locale || ''} onChange={e => patch({locale:e.target.value || null})}/></label>
            {editor.doc.sections.map((section, i) => <fieldset key={section.id} className="space-y-3 rounded-lg border p-4">
              <legend className="px-1 text-sm font-medium">章节 {i + 1}</legend>
              <div className="flex items-center gap-2"><label className="flex-1"><span className="sr-only">章节 {i+1} 标题</span><input className={field} placeholder="章节标题" value={section.title} onChange={e => patchSection(i,{title:e.target.value})}/></label>
                <Button type="button" variant="ghost" size="sm" aria-label={`上移章节 ${i+1}`} disabled={i===0} onClick={() => move(i,-1)}><ArrowUp size={16}/></Button>
                <Button type="button" variant="ghost" size="sm" aria-label={`下移章节 ${i+1}`} disabled={i===editor.doc.sections.length-1} onClick={() => move(i,1)}><ArrowDown size={16}/></Button>
                <Button type="button" variant="ghost" size="sm" aria-label={`删除章节 ${i+1}`} disabled={editor.doc.sections.length===1} onClick={() => patch({sections:editor.doc.sections.filter((_,j)=>j!==i)})}><Trash2 size={16}/></Button></div>
              <label className="block space-y-1"><span className="text-sm">生成要求</span><textarea className={field} rows={3} value={section.instruction} onChange={e => patchSection(i,{instruction:e.target.value})}/></label>
              <div className="grid gap-3 sm:grid-cols-2"><label className="space-y-1"><span className="text-sm">内容格式</span><select className={field} value={section.format} onChange={e=>patchSection(i,{format:e.target.value as TemplateSection['format']})}><option value="paragraph">段落</option><option value="list">列表 / 表格</option><option value="string">短文本</option></select></label>
                <label className="space-y-1"><span className="text-sm">没有相关内容时</span><select className={field} value={section.empty_behavior} onChange={e=>patchSection(i,{empty_behavior:e.target.value as TemplateSection['empty_behavior']})}><option value="show_not_mentioned">标记为未提及</option><option value="omit">省略（仅非必需章节）</option></select></label></div>
              <label className="flex gap-2 text-sm"><input type="checkbox" checked={section.required} onChange={e=>patchSection(i,{required:e.target.checked})}/>必需章节</label>
              <label className="block space-y-1"><span className="text-sm">格式提示（可选，支持 Markdown 表头）</span><textarea className={field} rows={2} value={section.item_format || section.example_item_format || ''} onChange={e=>patchSection(i,{item_format:e.target.value || null,example_item_format:null})}/></label>
            </fieldset>)}
            {!editor.readonly && <Button type="button" variant="outline" disabled={editor.doc.sections.length>=50} onClick={()=>patch({sections:[...editor.doc.sections,newSection()]})}><Plus size={16}/>添加章节</Button>}
          </fieldset>
          {discard && <div role="alert" className="rounded-md bg-amber-50 p-3 text-sm">有尚未保存的修改。<Button variant="ghost" onClick={()=>{setEditor(null);setDiscard(false);setError('');}}>放弃修改</Button><Button variant="ghost" onClick={()=>setDiscard(false)}>继续编辑</Button></div>}
          </div>
          <div className="flex shrink-0 justify-end gap-2 border-t border-gray-100 pt-4"><Button variant="outline" disabled={busy} onClick={closeEditor}>关闭</Button>{!editor.readonly && <Button disabled={busy} onClick={()=>void run(async()=>{
            const validation=validateTemplateDraft(editor.doc);if(validation)throw Error(validation);
            await templateLibrary.save(editor.doc,editor.expectedVersion);setEditor(null);await changed('模板已保存');
          })}>{busy?'保存中…':'保存模板'}</Button>}</div>
        </>}
      </DialogContent>
    </Dialog>
    <Dialog open={!!deleting} onOpenChange={open=>{if(!open&&!busy)setDeleting(null);}}><DialogContent><DialogTitle>删除自定义模板</DialogTitle><DialogDescription>确认删除“{deleting?.name}”？默认模板或被会议引用的模板需要先改选后才能删除。</DialogDescription>{error&&<p role="alert" className="text-red-700">{error}</p>}<div className="flex justify-end gap-2"><Button variant="outline" disabled={busy} onClick={()=>setDeleting(null)}>取消</Button><Button variant="destructive" disabled={busy} onClick={()=>void run(async()=>{if(!deleting)return;await templateLibrary.remove(deleting.id);setDeleting(null);await changed('模板已删除');})}>确认删除</Button></div></DialogContent></Dialog>
  </section>;
}
