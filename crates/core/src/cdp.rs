//! [`CdpLaunch`]: what the app did with Chromium's remote debugging (CDP) port at launch.
//!
//! The CDP port and a stealth identity are never combined: an open debugging port is easy for
//! sites and local software to detect, which defeats the identity. The app checks this at
//! startup (no port while a profile has an identity); while the port is open, every command
//! that gives a profile an identity calls [`CdpLaunch::check_stealth_allowed`]. It lives in
//! core because those commands are in crates (identity, profiles) the API crate depends on.

use crate::{CmdError, ErrorCode, Extensions};

/// What the app did with the CDP port at launch. Put into the registry extensions by the app;
/// absent = the port is closed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CdpLaunch {
    /// The port Chromium listens on, if any.
    pub port: Option<u16>,
    /// A port was asked for (flag or setting) but refused because of a stealth identity.
    pub refused: bool,
    /// The port was asked for with `--cdp-port` (not the saved setting): the flag decides at
    /// every start, whatever Settings say.
    pub from_cli: bool,
}

impl CdpLaunch {
    /// The launch facts of this browser (closed when the app did not insert any).
    pub fn of(extensions: &Extensions) -> CdpLaunch {
        extensions
            .get::<CdpLaunch>()
            .map(|l| (*l).clone())
            .unwrap_or_default()
    }

    /// Refuse to give a profile a stealth identity while the CDP port is open.
    pub fn check_stealth_allowed(&self) -> Result<(), CmdError> {
        let Some(port) = self.port else {
            return Ok(());
        };
        let how = if self.from_cli {
            "start the browser without --cdp-port"
        } else {
            "turn the CDP port off in Settings → Connections & API (api_config \
             {cdp_enabled: false}) and restart the browser"
        };
        Err(CmdError::new(
            ErrorCode::InvalidParams,
            format!(
                "a stealth identity cannot be used while the CDP port {port} is open \
                 (an open debugging port is easy for sites to detect)"
            ),
        )
        .with_hint(format!("To use an identity, {how}.")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stealth_is_refused_only_while_the_port_is_open() {
        assert!(CdpLaunch::default().check_stealth_allowed().is_ok());
        let refused = CdpLaunch {
            port: None,
            refused: true,
            from_cli: false,
        };
        assert!(refused.check_stealth_allowed().is_ok());

        let setting = CdpLaunch {
            port: Some(9222),
            ..Default::default()
        };
        let err = setting.check_stealth_allowed().unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
        assert!(err.message.contains("9222"), "{}", err.message);
        assert!(format!("{err:?}").contains("Settings"));

        let flag = CdpLaunch {
            port: Some(9333),
            from_cli: true,
            ..Default::default()
        };
        assert!(format!("{:?}", flag.check_stealth_allowed().unwrap_err()).contains("--cdp-port"));
    }

    #[test]
    fn missing_extension_means_closed() {
        let ext = Extensions::new();
        assert_eq!(CdpLaunch::of(&ext), CdpLaunch::default());
        ext.insert(CdpLaunch {
            port: Some(1),
            ..Default::default()
        });
        assert_eq!(CdpLaunch::of(&ext).port, Some(1));
    }
}
