//! API Key 保存在 Windows 凭据管理器中（服务名 ChronicleGM），不写入数据库或日志。

use chronicle_llm::secrets::SecretStore;

const SERVICE: &str = "ChronicleGM";

pub struct KeyringSecrets;

fn entry(id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, &format!("provider:{id}")).map_err(|e| format!("无法访问凭据管理器: {e}"))
}

impl SecretStore for KeyringSecrets {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        match entry(id)?.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("读取凭据失败: {e}")),
        }
    }
    fn set(&self, id: &str, secret: &str) -> Result<(), String> {
        entry(id)?.set_password(secret).map_err(|e| format!("保存凭据失败: {e}"))
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        match entry(id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("删除凭据失败: {e}")),
        }
    }
}
