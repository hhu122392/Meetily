import { describe, expect, test } from 'bun:test';
import { larkCredentialsUrl, parseLarkCredentials } from '../../src/lib/lark-credentials';

const secret = 'TestOnlySecret0123456789abcdefABCD';

describe('Lark 后台凭证识别', () => {
  test('支持分别复制、带标签的两项和 JSON 配置', () => {
    expect(parseLarkCredentials(' cli_test123 ')).toEqual({ appId: 'cli_test123' });
    expect(parseLarkCredentials(secret)).toEqual({ secret });
    expect(parseLarkCredentials(`App ID\r\ncli_test123\r\nApp Secret\r\n${secret}`)).toEqual({ appId: 'cli_test123', secret });
    expect(parseLarkCredentials(`App ID: cli_test123\nApp Secret：${secret}`)).toEqual({ appId: 'cli_test123', secret });
    expect(parseLarkCredentials(JSON.stringify({ app_id: 'cli_test123', app_secret: secret }))).toEqual({ appId: 'cli_test123', secret });
  });

  test('拒绝遮挡文字、无关文本、多组凭证和错误类型，错误不泄露原文', () => {
    for (const input of ['********', '∗∗∗∗∗∗', 'App Secret\n********', 'App ID\ncli_test123\nApp Secret\n∗∗',
      'cli_', 'https://example.com/' + secret, '复制', '{"app_id":123}',
      `App ID: cli_one\nApp ID: cli_two\nApp Secret: ${secret}`, 'x'.repeat(8193)]) {
      let error = '';
      try { parseLarkCredentials(input); } catch (e) { error = (e as Error).message; }
      expect(error).not.toBe('');
      expect(error).not.toContain(secret);
    }
  });

  test('后台地址只能来自官方域名，不把密钥或任意 URL 拼入地址', () => {
    expect(larkCredentialsUrl('cli_test123')).toBe('https://open.larksuite.com/app/cli_test123/baseinfo');
    for (const input of ['', secret, 'cli_a/../../evil', 'https://evil.example']) {
      expect(larkCredentialsUrl(input)).toBe('https://open.larksuite.com/app');
    }
  });
});
