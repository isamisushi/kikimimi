//! Installation transaction shared with the portable regression tests.
pub(crate) trait Host {
    /// Must return only cryptographically verified archive bytes.
    async fn download_verified(&mut self) -> Result<Vec<u8>, String>;
    fn mark_pending(&mut self) -> Result<(), String>;
    /// Must leave the old bundle in place if installation fails.
    fn install_atomically(&mut self, bytes: Vec<u8>) -> Result<(), String>;
    async fn restart_collector(&mut self) -> Result<(), String>;
    fn clear_pending(&mut self) -> Result<(), String>;
}

pub(crate) async fn apply(host: &mut impl Host) -> Result<(), String> {
    let bytes = host.download_verified().await?;
    host.mark_pending()?;
    if let Err(error) = host.install_atomically(bytes) {
        host.clear_pending()?;
        return Err(error);
    }
    // Keep the marker if restarting fails, so the next launch can retry.
    host.restart_collector().await?;
    host.clear_pending()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        fail: &'static str,
        calls: Vec<&'static str>,
        pending: bool,
    }
    impl Fake {
        fn step(&mut self, name: &'static str) -> Result<(), String> {
            self.calls.push(name);
            if self.fail == name {
                Err(name.into())
            } else {
                Ok(())
            }
        }
    }
    impl Host for Fake {
        async fn download_verified(&mut self) -> Result<Vec<u8>, String> {
            self.step("verify")?;
            Ok(vec![1])
        }
        fn mark_pending(&mut self) -> Result<(), String> {
            self.step("mark")?;
            self.pending = true;
            Ok(())
        }
        fn install_atomically(&mut self, _: Vec<u8>) -> Result<(), String> {
            self.step("install")
        }
        async fn restart_collector(&mut self) -> Result<(), String> {
            self.step("restart").map_err(|_| "restart".into())
        }
        fn clear_pending(&mut self) -> Result<(), String> {
            self.step("clear")?;
            self.pending = false;
            Ok(())
        }
    }
    #[tokio::test]
    async fn rejected_signature_never_installs_or_restarts() {
        let mut host = Fake {
            fail: "verify",
            calls: vec![],
            pending: false,
        };
        assert!(apply(&mut host).await.is_err());
        assert_eq!(host.calls, ["verify"]);
        assert!(!host.pending);
    }
    #[tokio::test]
    async fn failed_install_keeps_collector_and_clears_marker() {
        let mut host = Fake {
            fail: "install",
            calls: vec![],
            pending: false,
        };
        assert!(apply(&mut host).await.is_err());
        assert_eq!(host.calls, ["verify", "mark", "install", "clear"]);
        assert!(!host.pending);
    }
    #[tokio::test]
    async fn failed_restart_keeps_recovery_marker() {
        let mut host = Fake {
            fail: "restart",
            calls: vec![],
            pending: false,
        };
        assert!(apply(&mut host).await.is_err());
        assert!(host.pending);
    }
    #[tokio::test]
    async fn successful_update_restarts_only_after_installation() {
        let mut host = Fake {
            fail: "",
            calls: vec![],
            pending: false,
        };
        apply(&mut host).await.unwrap();
        assert_eq!(
            host.calls,
            ["verify", "mark", "install", "restart", "clear"]
        );
        assert!(!host.pending);
    }
    #[tokio::test]
    async fn cannot_install_without_durable_recovery_marker() {
        let mut host = Fake {
            fail: "mark",
            calls: vec![],
            pending: false,
        };
        assert!(apply(&mut host).await.is_err());
        assert_eq!(host.calls, ["verify", "mark"]);
    }
}
