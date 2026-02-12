use crate::{crypto, store::AppStore};
use anyhow::{anyhow, Result};
use std::sync::Mutex;
use zeroize::Zeroize;

#[derive(Debug)]
pub struct VaultSession {
    key: Option<Vec<u8>>,
}

impl VaultSession {
    pub fn new() -> Self {
        Self { key: None }
    }

    pub fn set_key(&mut self, key: Vec<u8>) {
        self.lock();
        self.key = Some(key);
    }

    pub fn lock(&mut self) {
        if let Some(key) = self.key.as_mut() {
            key.zeroize();
        }
        self.key = None;
    }

    pub fn is_unlocked(&self) -> bool {
        self.key.is_some()
    }

    pub fn get_key(&self) -> Result<Vec<u8>> {
        self.key
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow!("保险库未解锁，请先输入主密码"))
    }
}

#[derive(Debug)]
pub struct AppState {
    pub store: AppStore,
    pub vault: Mutex<VaultSession>,
}

impl AppState {
    pub fn initialize(store: AppStore) -> Result<Self> {
        store.init()?;
        Ok(Self {
            store,
            vault: Mutex::new(VaultSession::new()),
        })
    }

    pub fn init_vault(&self, master_password: &str) -> Result<bool> {
        let settings = self.store.get_vault_settings()?;
        if settings.salt.is_some() {
            return Ok(false);
        }
        let salt = crypto::generate_salt();
        self.store.set_vault_salt(&salt)?;
        let key = crypto::derive_key(master_password, &salt)?;
        self.vault
            .lock()
            .map_err(|_| anyhow!("保险库状态锁失败"))?
            .set_key(key);
        Ok(true)
    }

    pub fn unlock_vault(&self, master_password: &str) -> Result<()> {
        let settings = self.store.get_vault_settings()?;
        let salt = settings
            .salt
            .ok_or_else(|| anyhow!("保险库尚未初始化，请先设置主密码"))?;
        let key = crypto::derive_key(master_password, &salt)?;
        self.vault
            .lock()
            .map_err(|_| anyhow!("保险库状态锁失败"))?
            .set_key(key);
        Ok(())
    }

    pub fn lock_vault(&self) -> Result<()> {
        self.vault
            .lock()
            .map_err(|_| anyhow!("保险库状态锁失败"))?
            .lock();
        Ok(())
    }

    pub fn is_vault_unlocked(&self) -> Result<bool> {
        Ok(self
            .vault
            .lock()
            .map_err(|_| anyhow!("保险库状态锁失败"))?
            .is_unlocked())
    }

    pub fn get_vault_key(&self) -> Result<Vec<u8>> {
        self.vault
            .lock()
            .map_err(|_| anyhow!("保险库状态锁失败"))?
            .get_key()
    }
}
