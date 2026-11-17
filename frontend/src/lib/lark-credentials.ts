export type LarkCredentials = { appId?: string; secret?: string };

export function isLarkAppId(value: string): boolean {
  return /^cli_[A-Za-z0-9_]+$/.test(value) && value.length <= 80;
}

export function larkCredentialsUrl(appId: string): string {
  const id = appId.trim();
  return isLarkAppId(id)
    ? `https://open.larksuite.com/app/${id}/baseinfo`
    : 'https://open.larksuite.com/app';
}

function isSecret(value: string): boolean {
  return /^[A-Za-z0-9_-]{16,512}$/.test(value) && !value.startsWith('cli_');
}

// Only accept explicit credentials, never infer a secret from arbitrary page text.
// Errors deliberately omit the input so credentials cannot appear in notifications.
export function parseLarkCredentials(input: string): LarkCredentials {
  const text = input.trim();
  if (!text || text.length > 8192) throw new Error('请复制 App ID 或 App Secret 后再粘贴。');
  if (isLarkAppId(text)) return { appId: text };
  if (isSecret(text)) return { secret: text };

  let appId: unknown;
  let secret: unknown;
  if (text.startsWith('{')) {
    try {
      const data = JSON.parse(text);
      appId = data.app_id ?? data.appId;
      secret = data.app_secret ?? data.appSecret ?? data.secret;
    } catch { throw new Error('无法识别凭证，请使用后台 App ID 和 App Secret 旁的复制按钮。'); }
  } else {
    const ids = [...text.matchAll(/(?:^|\n)[ \t]*App[ _]?ID[ \t]*(?:[:：][ \t]*)?(?:\r?\n[ \t]*)?([^\r\n]+)/gi)];
    const secrets = [...text.matchAll(/(?:^|\n)[ \t]*App[ _]?Secret[ \t]*(?:[:：][ \t]*)?(?:\r?\n[ \t]*)?([^\r\n]+)/gi)];
    if (ids.length > 1 || secrets.length > 1) throw new Error('检测到多组凭证，请只复制同一个应用的凭证。');
    appId = ids[0]?.[1].trim();
    secret = secrets[0]?.[1].trim();
  }
  if (appId !== undefined && (typeof appId !== 'string' || !isLarkAppId(appId))) {
    throw new Error('App ID 格式不正确，请使用后台 App ID 旁的复制按钮。');
  }
  if (secret !== undefined && (typeof secret !== 'string' || !isSecret(secret))) {
    throw new Error('密钥不完整或仍为星号，请使用 App Secret 旁的复制按钮，不要复制遮挡文字。');
  }
  if (appId === undefined && secret === undefined) {
    throw new Error('无法识别凭证，请使用后台 App ID 和 App Secret 旁的复制按钮。');
  }
  return { ...(appId !== undefined ? { appId: appId as string } : {}), ...(secret !== undefined ? { secret: secret as string } : {}) };
}
