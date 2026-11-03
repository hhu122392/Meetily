let activeCompany: string | null = null;

export function initializeCompanyScope(id: string) {
  if (!id) throw new Error('公司标识为空');
  if (activeCompany && activeCompany !== id) throw new Error('切换公司需要重启 Meetily');
  activeCompany = id;
}

export function companyStorageKey(key: string): string {
  if (!activeCompany) throw new Error('公司资料尚未加载');
  return storageKeyForCompany(activeCompany, key);
}

export function storageKeyForCompany(id: string, key: string): string {
  if (!id) throw new Error('公司标识为空');
  // Existing recovery drafts belong exclusively to the original company.
  return id === 'legacy' ? key : `${key}:company:${id}`;
}
