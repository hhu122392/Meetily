import {afterEach,expect,mock,test} from 'bun:test';
import {act,create,type ReactTestRenderer} from 'react-test-renderer';
let respond:(command:string,args:any)=>Promise<any>;
const invoke=mock(async(c:string,a:any)=>respond(c,a));
mock.module('@tauri-apps/api/core',()=>({invoke}));
const {LarkDirectoryPicker}=await import('../src/components/MeetingDetails/LarkDirectoryPicker');
let renderer:ReactTestRenderer;const selected=mock(async()=>{});
const button=(label:string)=>renderer.root.findAllByType('button').find(b=>b.children.filter(x=>typeof x==='string').join('').includes(label))!;
afterEach(async()=>{if(renderer)await act(async()=>renderer.unmount());invoke.mockClear();selected.mockClear();});
async function mount(){respond=async(c,a)=>{
  if(c==='lark_location_choose')return 'saved';
  if(c==='lark_directory_tree'){const item={token:'folder1',name:'战略会',space:'space1',origin:'https://team.larksuite.com'};return {items:[{...item,path:[item]}],complete:true,notice:'',cached:false,requests:1,updatedAt:1};}
  if(c==='lark_directory_list')return a.kind==='spaces'?{items:[{token:'space1',name:'项目知识库',space:'space1',origin:'https://larksuite.com'}],nextPageToken:''}:a.parent?{items:[],nextPageToken:''}:{items:[{token:'folder1',name:'战略会',space:'space1',origin:'https://team.larksuite.com'}],nextPageToken:''};
};await act(async()=>{renderer=create(<LarkDirectoryPicker onSelected={selected}/>);});}
test('仅点击时加载，展开知识库目录并明确选择后才保存位置',async()=>{
  await mount();expect(invoke).not.toHaveBeenCalled();
  await act(async()=>button('选择云空间或知识库目录').props.onClick());
  expect(button('保存到此目录').props.disabled).toBe(true);
  await act(async()=>button('📁 项目知识库').props.onClick());
  await act(async()=>button('📁 战略会').props.onClick());
  expect(invoke.mock.calls.some(c=>c[0]==='lark_location_choose')).toBe(false);
  await act(async()=>button('保存到此目录').props.onClick());
  expect(invoke.mock.calls.find(c=>c[0]==='lark_location_choose')?.[1].target).toMatchObject({kind:'wiki',space:'space1',token:'folder1',name:'知识库 / 项目知识库 / 战略会'});
  expect(selected).toHaveBeenCalledWith('saved');
});
test('云空间可选根目录和子目录，不创建文档或读取群',async()=>{
  await mount();await act(async()=>button('选择云空间或知识库目录').props.onClick());
  await act(async()=>renderer.root.findAllByType('button').find(b=>b.children.includes('云空间'))!.props.onClick());
  await act(async()=>button('保存到此目录').props.onClick());
  expect(invoke.mock.calls.find(c=>c[0]==='lark_location_choose')?.[1].target).toMatchObject({kind:'drive',token:'',space:''});
  expect(invoke.mock.calls.some(c=>['lark_document_save','lark_manual_send','lark_manual_groups'].includes(c[0]))).toBe(false);
});
test('空页仍可继续分页，返回已加载目录使用缓存',async()=>{
  await mount();respond=async()=>({items:[],nextPageToken:'next'});
  await act(async()=>button('选择云空间或知识库目录').props.onClick());
  expect(button('加载更多目录')).toBeDefined();
  respond=async()=>({items:[{token:'s',name:'库',space:'s',origin:'https://larksuite.com'}],nextPageToken:''});
  await act(async()=>button('加载更多目录').props.onClick());
  expect(invoke.mock.calls.filter(c=>c[0]==='lark_directory_list')[1][1].pageToken).toBe('next');
  const before=invoke.mock.calls.length;
  await act(async()=>renderer.root.findByProps({'aria-label':'目录路径'}).findByType('button').props.onClick());
  expect(invoke.mock.calls.length).toBe(before);
});
test('权限失败不允许选目录，连接云空间只打开授权，不发送消息',async()=>{
  await mount();respond=async(c)=>{if(c==='lark_app_authorize')return 'https://accounts.larksuite.com/auth';if(c==='open_external_url')return;throw '目录无权限';};
  await act(async()=>button('选择云空间或知识库目录').props.onClick());
  await act(async()=>renderer.root.findAllByType('button').find(b=>b.children.includes('云空间'))!.props.onClick());
  expect(button('保存到此目录').props.disabled).toBe(true);
  await act(async()=>button('连接我的云空间').props.onClick());
  expect(invoke.mock.calls).toContainEqual(['lark_app_authorize',{documentsOnly:true}]);
  expect(selected).not.toHaveBeenCalled();
});

test('进入知识库自动整库加载，搜索深层目录并选择不再逐级读取',async()=>{
  await mount();const previous=respond;
  const parent={token:'parent',name:'项目',space:'space1',origin:'https://team.larksuite.com'};
  const child={token:'deep',name:'战略周会',space:'space1',origin:'https://team.larksuite.com'};
  respond=async(c,a)=>c==='lark_directory_tree'?{items:[{...parent,path:[parent]},{...child,path:[parent,child],hasChildren:false}],complete:true,notice:'',cached:false,requests:2,updatedAt:1}:previous(c,a);
  await act(async()=>button('选择云空间或知识库目录').props.onClick());await act(async()=>button('📁 项目知识库').props.onClick());
  expect(button('一键加载本知识库全部目录')).toBeUndefined();
  expect(invoke.mock.calls.filter(c=>c[0]==='lark_directory_tree')).toHaveLength(1);
  const before=invoke.mock.calls.filter(c=>c[0]==='lark_directory_list').length;
  await act(async()=>renderer.root.findByProps({'aria-label':'搜索知识库目录'}).props.onChange({target:{value:'战略'}}));
  const node=renderer.root.findAllByProps({role:'treeitem'});expect(node).toHaveLength(1);
  await act(async()=>node[0].props.onClick());
  expect(invoke.mock.calls.filter(c=>c[0]==='lark_directory_list')).toHaveLength(before);
  await act(async()=>renderer.root.findByProps({'aria-label':'目录路径'}).findAllByType('button')[1].props.onClick());
  expect(invoke.mock.calls.filter(c=>c[0]==='lark_directory_tree')).toHaveLength(1);
  await act(async()=>node[0].props.onClick());
  expect(invoke.mock.calls.some(c=>c[0]==='lark_location_choose')).toBe(false);
  await act(async()=>button('保存到此目录').props.onClick());
  expect(invoke.mock.calls.find(c=>c[0]==='lark_location_choose')?.[1].target).toEqual({kind:'wiki',space:'space1',token:'deep',origin:'https://team.larksuite.com',name:'知识库 / 项目知识库 / 项目 / 战略周会'});
});
test('有已保存位置也自动读取知识库列表，重开复用缓存且可直接使用上次位置',async()=>{
  await mount();const previous=respond;
  respond=async(c,a)=>c==='lark_location_context'?{projects:[{name:'saved',label:'战略会'}],lastProject:'saved',cacheScope:'scope'}:previous(c,a);
  await act(async()=>button('选择云空间或知识库目录').props.onClick());
  expect(invoke.mock.calls.filter(c=>c[0]==='lark_directory_list')).toHaveLength(1);
  await act(async()=>button('收起目录').props.onClick());const count=invoke.mock.calls.filter(c=>c[0]==='lark_directory_list').length;
  await act(async()=>button('选择云空间或知识库目录').props.onClick());
  expect(invoke.mock.calls.filter(c=>c[0]==='lark_directory_list')).toHaveLength(count);
  await act(async()=>button('使用上次保存位置').props.onClick());expect(selected).toHaveBeenCalledWith('saved');expect(invoke.mock.calls.some(c=>c[0]==='lark_directory_tree')).toBe(false);
});
test('整库加载可取消，部分结果明确标注，账户变化会清理浏览缓存',async()=>{
  await mount();const previous=respond;let finish:(v:any)=>void=()=>{};let account='a';
  respond=async(c,a)=>{
    if(c==='lark_location_context')return {projects:[],cacheScope:account};
    if(c==='lark_directory_tree')return new Promise(resolve=>{finish=resolve;});
    if(c==='lark_directory_cancel'){finish({items:[],complete:false,notice:'已取消，保留部分结果',cached:false,requests:1,updatedAt:1});return;}
    return previous(c,a);
  };
  await act(async()=>button('选择云空间或知识库目录').props.onClick());
  await act(async()=>{button('📁 项目知识库').props.onClick();await Promise.resolve();});
  expect(button('取消加载')).toBeDefined();await act(async()=>button('取消加载').props.onClick());
  expect(JSON.stringify(renderer.toJSON())).toContain('部分目录');
  await act(async()=>button('收起目录').props.onClick());account='b';const before=invoke.mock.calls.filter(c=>c[0]==='lark_directory_list').length;
  await act(async()=>button('选择云空间或知识库目录').props.onClick());expect(invoke.mock.calls.filter(c=>c[0]==='lark_directory_list')).toHaveLength(before+1);
});

test('自动整库加载失败不循环重试，用户刷新可恢复且不保存位置',async()=>{
  await mount();const previous=respond;let fail=true;
  respond=async(c,a)=>{if(c==='lark_directory_tree'&&fail)throw '读取目录失败';return previous(c,a);};
  await act(async()=>button('选择云空间或知识库目录').props.onClick());
  await act(async()=>button('📁 项目知识库').props.onClick());
  expect(renderer.root.findByProps({role:'alert'}).children.join('')).toContain('读取目录失败');
  expect(button('保存到此目录').props.disabled).toBe(true);
  expect(invoke.mock.calls.filter(c=>c[0]==='lark_directory_tree')).toHaveLength(1);
  fail=false;await act(async()=>button('刷新全部目录').props.onClick());
  expect(renderer.root.findAllByProps({role:'treeitem'})).toHaveLength(1);
  expect(invoke.mock.calls.filter(c=>c[0]==='lark_directory_tree').at(-1)?.[1].force).toBe(true);
  expect(selected).not.toHaveBeenCalled();
});
