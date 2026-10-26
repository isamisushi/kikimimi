//! `config.json` の秘密情報 (device token と Codex 用の router key) を macOS の login
//! Keychain に置く。`config.json` 側の値は空文字列になり、空なら Keychain から読む
//! (`crate::config`)。
//!
//! 0600 のファイルは他のユーザーからは守れるが、同じユーザーで動くプロセス (npm の
//! postinstall、エディタの拡張、AI エージェントが走らせるシェルコマンド) からは黙って
//! 読める。Keychain の項目は作ったバイナリのコード署名に紐づくので、それ以外の
//! プログラムが読むときは macOS が確認を出す。同じ Developer ID で署名した更新版は
//! そのまま読める。
//!
//! 項目は service `kikimimi`、account `<名前>:<config.json のあるディレクトリ>`。
//! `KIKIMIMI_DIR` ごとに別の項目になる。
//!
//! macOS 以外と `KIKIMIMI_SECRET_STORE=file` のときは使わない (今までどおり 0600 の
//! ファイルに置く)。テストでは本物の Keychain の代わりにプロセス内の表を使う。

use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub enum Secret {
    DeviceToken,
    CodexRouterKey,
}

impl Secret {
    fn name(self) -> &'static str {
        match self {
            Secret::DeviceToken => "device-token",
            Secret::CodexRouterKey => "codex-router-key",
        }
    }
}

fn account(secret: Secret, dir: &Path) -> String {
    format!("{}:{}", secret.name(), dir.display())
}

pub fn enabled() -> bool {
    backend::AVAILABLE && std::env::var("KIKIMIMI_SECRET_STORE").as_deref() != Ok("file")
}

/// `Ok(None)` when there is no item.
pub fn get(secret: Secret, dir: &Path) -> anyhow::Result<Option<String>> {
    backend::get(&account(secret, dir))
}

pub fn set(secret: Secret, dir: &Path, value: &str) -> anyhow::Result<()> {
    backend::set(&account(secret, dir), value)
}

/// Deleting an item that does not exist is not an error.
pub fn delete(secret: Secret, dir: &Path) -> anyhow::Result<()> {
    backend::delete(&account(secret, dir))
}

#[cfg(all(target_os = "macos", not(test)))]
mod backend {
    use anyhow::Context;
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    pub const AVAILABLE: bool = true;
    const SERVICE: &str = "kikimimi";
    // errSecItemNotFound
    const NOT_FOUND: i32 = -25300;

    pub fn get(account: &str) -> anyhow::Result<Option<String>> {
        match get_generic_password(SERVICE, account) {
            Ok(bytes) => Ok(Some(
                String::from_utf8(bytes).context("Keychain item is not UTF-8")?,
            )),
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            Err(e) => Err(e).context("reading the Keychain"),
        }
    }

    pub fn set(account: &str, value: &str) -> anyhow::Result<()> {
        set_generic_password(SERVICE, account, value.as_bytes()).context("writing the Keychain")
    }

    pub fn delete(account: &str) -> anyhow::Result<()> {
        match delete_generic_password(SERVICE, account) {
            Err(e) if e.code() != NOT_FOUND => Err(e).context("deleting from the Keychain"),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod backend {
    use std::collections::HashMap;
    use std::sync::Mutex;

    pub const AVAILABLE: bool = true;
    static ITEMS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

    pub fn get(account: &str) -> anyhow::Result<Option<String>> {
        Ok(ITEMS
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|m| m.get(account).cloned()))
    }

    pub fn set(account: &str, value: &str) -> anyhow::Result<()> {
        ITEMS
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(account.to_string(), value.to_string());
        Ok(())
    }

    pub fn delete(account: &str) -> anyhow::Result<()> {
        if let Some(m) = ITEMS.lock().unwrap().as_mut() {
            m.remove(account);
        }
        Ok(())
    }
}

#[cfg(all(not(target_os = "macos"), not(test)))]
mod backend {
    pub const AVAILABLE: bool = false;

    pub fn get(_: &str) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    pub fn set(_: &str, _: &str) -> anyhow::Result<()> {
        anyhow::bail!("no Keychain on this platform")
    }

    pub fn delete(_: &str) -> anyhow::Result<()> {
        Ok(())
    }
}
