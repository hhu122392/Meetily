use base64::{engine::general_purpose::STANDARD, Engine};
use ring::{aead, rand::{SecureRandom, SystemRandom}};

const PREFIX: &str = "mac-keychain-v1:";
const AAD: &[u8] = b"com.meetily.ai/lark/config/v1";

fn seal(value: &str, key: &[u8]) -> Result<String, String> {
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, key)
        .map_err(|_| "Mac 凭证密钥格式异常。")?);
    let mut nonce = [0u8; 12];
    SystemRandom::new().fill(&mut nonce).map_err(|_| "无法生成凭证随机数。")?;
    let mut data = value.as_bytes().to_vec();
    key.seal_in_place_append_tag(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::from(AAD), &mut data)
        .map_err(|_| "Mac 凭证加密失败。")?;
    let mut payload = nonce.to_vec();
    payload.extend(data);
    Ok(format!("{PREFIX}{}", STANDARD.encode(payload)))
}

fn open(value: &str, key: &[u8]) -> Result<String, String> {
    let payload = value.strip_prefix(PREFIX).ok_or("该凭证不是本机 Mac 格式，请重新配置并授权 Lark。")?;
    let mut data = STANDARD.decode(payload).map_err(|_| "Mac 凭证格式异常。")?;
    if data.len() < 12 + aead::AES_256_GCM.tag_len() { return Err("Mac 凭证数据不完整。".into()); }
    let nonce = aead::Nonce::try_assume_unique_for_key(&data[..12]).map_err(|_| "Mac 凭证随机数格式异常。")?;
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, key)
        .map_err(|_| "Mac 凭证密钥格式异常。")?);
    let plain = key.open_in_place(nonce, aead::Aad::from(AAD), &mut data[12..])
        .map_err(|_| "无法解密 Lark 凭证，请使用原 Mac 用户或重新配置并授权。")?;
    String::from_utf8(plain.to_vec()).map_err(|_| "Mac 凭证编码异常。".into())
}

#[cfg(target_os = "macos")]
pub(super) fn protect(value: &str, decrypt: bool) -> Result<String, String> {
    use security_framework::passwords::{generic_password, set_generic_password, PasswordOptions};
    // Serialize first creation across company profiles; the app also has a single-instance guard.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    const SERVICE: &str = "com.meetily.ai.lark.credentials";
    const ACCOUNT: &str = "encryption-key-v1";
    let _guard = LOCK.lock().map_err(|_| "Mac 凭证保护暂不可用，请重启应用。")?;
    if decrypt && !value.starts_with(PREFIX) {
        return Err("该凭证不是本机 Mac 格式，请重新配置并授权 Lark。".into());
    }
    let mut key = match generic_password(PasswordOptions::new_generic_password(SERVICE, ACCOUNT)) {
        Ok(key) => key,
        Err(error) if error.code() == -25300 && !decrypt => {
            let mut key = vec![0u8; 32];
            SystemRandom::new().fill(&mut key).map_err(|_| "无法生成 Mac 凭证密钥。")?;
            set_generic_password(SERVICE, ACCOUNT, &key)
                .map_err(|_| "无法保存到 Mac 钥匙串，请解锁登录钥匙串并允许 Meetily 访问。")?;
            key
        }
        Err(_) => return Err("无法读取 Mac 钥匙串中的 Lark 密钥，请解锁钥匙串或重新配置并授权。".into()),
    };
    // Only authenticated ciphertext is written to SQLite; the key stays in the login keychain.
    let result = if decrypt { open(value, &key) } else { seal(value, &key) };
    key.fill(0);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mac_credentials_round_trip_and_random_nonce() {
        let key = [7u8; 32];
        let value = r#"{"secret":"测试密钥","refresh_token":"token"}"#;
        let a = seal(value, &key).unwrap();
        let b = seal(value, &key).unwrap();
        assert_ne!(a, b);
        assert!(!a.contains("测试密钥"));
        assert_eq!(open(&a, &key).unwrap(), value);
        assert_eq!(open(&b, &key).unwrap(), value);
    }
    #[test]
    fn mac_credentials_reject_wrong_key_and_tampering() {
        let original = seal("secret", &[7u8; 32]).unwrap();
        assert!(open(&original, &[8u8; 32]).is_err());
        let mut data = STANDARD.decode(original.strip_prefix(PREFIX).unwrap()).unwrap();
        data[12] ^= 1;
        assert!(open(&format!("{PREFIX}{}", STANDARD.encode(data)), &[7u8; 32]).is_err());
    }
    #[test]
    fn mac_credentials_reject_legacy_truncated_and_invalid_data() {
        for invalid in ["", "windows-dpapi", "mac-keychain-v1:!", "mac-keychain-v1:YWJj"] {
            assert!(open(invalid, &[7u8; 32]).is_err());
        }
        assert!(seal("secret", &[0u8; 10]).is_err());
    }
}
