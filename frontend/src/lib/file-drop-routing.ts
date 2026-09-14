const TEMPLATE_IMPORT_EXTENSIONS = new Set(['json', 'docx', 'doc']);

function extensionOf(path: string): string {
  return path.split('.').at(-1)?.toLocaleLowerCase() ?? '';
}

export function shouldDelegateDropToTemplateImport(
  pathname: string,
  paths: readonly string[],
): boolean {
  return pathname === '/settings/templates'
    && paths.some((path) => TEMPLATE_IMPORT_EXTENSIONS.has(extensionOf(path)));
}
