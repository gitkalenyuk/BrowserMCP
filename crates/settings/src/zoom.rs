//! Per-site zoom levels (`data/config/zoom.json`, kept by the CEF layer): `zoom.list`,
//! `zoom.reset`. Changing a site's zoom is `page.zoom` on one of its tabs.

use bmcp_core::{CmdError, Command, Ctx, Meta, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SiteZoom {
    /// Host name, e.g. `example.com`.
    pub host: String,
    /// Zoom in percent (100 = normal; such hosts are not listed).
    pub percent: f64,
}

pub struct List;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListOutput {
    /// Sorted by host.
    pub sites: Vec<SiteZoom>,
}

impl Command for List {
    const NAME: &'static str = "zoom.list";
    type Args = bmcp_core::command::Empty;
    type Output = ListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List the sites with a remembered page zoom other than 100 % (set with page_zoom or \
             Ctrl+=/-, saved per host in data/config/zoom.json). Returns {sites:[{host, percent}]}.",
            Toolset::System,
        )
    }

    async fn run(ctx: Ctx, _args: bmcp_core::command::Empty) -> Result<ListOutput, CmdError> {
        let mut sites: Vec<SiteZoom> = ctx
            .backend
            .zoom_levels()
            .await?
            .into_iter()
            .map(|(host, percent)| SiteZoom { host, percent })
            .collect();
        sites.sort_by(|a, b| a.host.cmp(&b.host));
        Ok(ListOutput { sites })
    }
}

pub struct Reset;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResetArgs {
    /// Host to reset (e.g. "example.com"); omitted = every site.
    pub host: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ResetOutput {
    /// Hosts whose zoom went back to 100 %.
    pub reset: Vec<String>,
}

impl Command for Reset {
    const NAME: &'static str = "zoom.reset";
    type Args = ResetArgs;
    type Output = ResetOutput;

    fn meta() -> Meta {
        Meta::new(
            "Forget the remembered page zoom of a site (host) or of every site; its open tabs \
             go back to 100 % at once. Returns the hosts that were reset.",
            Toolset::System,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ResetArgs) -> Result<ResetOutput, CmdError> {
        let host = args
            .host
            .map(|h| h.trim().to_ascii_lowercase())
            .filter(|h| !h.is_empty());
        Ok(ResetOutput {
            reset: ctx.backend.reset_zoom(host).await?,
        })
    }
}
