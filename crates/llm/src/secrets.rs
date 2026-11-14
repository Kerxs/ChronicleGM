//! API Key 存储接口。正式实现使用 Windows 凭据管理器（在应用层），测试使用内存实现。
//! Key 不写入数据库、配置文件或日志。

use std::collections::HashMap;
use std::sync::Mutex;

pub trait SecretStore: Send + Sync {
    fn get(&self, id: &str) -> Result<Option<String>, String>;
    fn set(&self, id: &str, secret: &str) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<(), String>;
}

#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, id: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().map_err(|e| e.to_string())?.get(id).cloned())
    }
    fn set(&self, id: &str, secret: &str) -> Result<(), String> {
        self.0.lock().map_err(|e| e.to_string())?.insert(id.into(), secret.into());
        Ok(())
    }
    fn delete(&self, id: &str) -> Result<(), String> {
        self.0.lock().map_err(|e| e.to_string())?.remove(id);
        Ok(())
    }
}
